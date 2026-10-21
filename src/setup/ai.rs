//! The guided setup's AI step: the user's own words about their world ("a cold northern
//! continent, jungle islands in the south, a dwarf capital in the mountains") are read into the
//! plan by the AI the user connected (Prompt an area's backends: Codex, Claude Code or an API
//! key). It answers with the plan fields to change and names for the world, its capital, towns,
//! zones, dungeons and realms. No tools: it only reads and writes JSON.

use std::path::PathBuf;
use std::time::Duration;

use bevy::prelude::*;
use serde_json::Value;

use super::Setup;
use super::plan::{INSPIRATIONS, WorldPlan};
use crate::prompt::agent::{self, AgentEvent, AgentHandle, Backend};

const INSTRUCTIONS: &str = "You help someone set up the world of their MMO in Skyfold Studio, a world builder. \
You get their own description of the world and the choices they made so far (a JSON plan). Read the description and answer with ONE JSON object and nothing else (no prose, no code fence):
{\"plan\": { only the fields the description asks to change, with the same names and kinds of value as the plan you were given }, \"names\": {\"world\": \"…\", \"capital\": \"…\", \"towns\": [\"…\"], \"zones\": [\"…\"], \"dungeons\": [\"…\"], \"realms\": [\"…\"]}, \"understood\": \"one or two plain sentences telling the user what you took from their words\"}
Rules:
- Change a field only when the words say so (\"three realms at war\" → realms 3 and game realm_war; \"lots of mining\" → gathering.ore true and a higher gathering.density; \"huge\" → size large; \"islands\" → land archipelago; \"frozen north and deserts\" → climate varied).
- Allowed values: art hand_painted | photoreal; game themepark | sandbox | realm_war | survival | story; land continent | twin_islands | archipelago | peninsula | inland_sea | fjord_coast | mountain_island; size small | medium | large; climate varied | temperate | tropical | arid | cold; look natural | nordic | cinematic | epic_sunbeams | overcast | lavender | golden_vintage | rose_haze | hellfire | enchanted | golden_hour.
- Always make up names that fit their world and its mood: a world name, a capital, one name per town (towns + villages in the plan), about one zone name per town, one name per dungeon and raid, one per realm. Short, evocative, original (never names from existing games or books).
- Never put anything but the JSON object in your answer.";

/// The AI run reading the description.
#[derive(Resource, Default)]
pub struct AiRun {
    handle: Option<AgentHandle>,
    endpoint: Option<crate::mcp::PrivateEndpoint>,
    workdir: Option<PathBuf>,
    /// the plan as it was asked about
    asked: Option<WorldPlan>,
}

/// Starts reading `Setup.plan.description` with the backend chosen in Prompt an area.
pub fn start(world: &mut World) {
    world.init_resource::<AiRun>();
    if world.resource::<AiRun>().handle.is_some() {
        return;
    }
    let plan = world.resource::<Setup>().plan.clone();
    let fail = |world: &mut World, e: String| {
        let mut s = world.resource_mut::<Setup>();
        s.ai.running = false;
        s.ai.error = Some(e);
    };
    if plan.description.trim().is_empty() {
        return fail(world, "Describe your world first".into());
    }
    let (backend, model, effort, status, key) = {
        let s = world.resource::<crate::prompt::PromptSession>();
        let key = Some(s.api_key.trim().to_string())
            .filter(|k| !k.is_empty())
            .or_else(agent::env_api_key);
        (
            s.backend,
            s.model(),
            s.effort(),
            s.status(s.backend).cloned(),
            key,
        )
    };
    if backend != Backend::AnthropicApi
        && let Some(st) = &status
        && !st.ready
    {
        return fail(world, format!("{}: {}", backend.name(), st.note));
    }
    if backend == Backend::AnthropicApi && key.is_none() {
        return fail(
            world,
            "Enter an Anthropic API key first (or connect Codex or Claude Code)".into(),
        );
    }
    let endpoint = match crate::mcp::PrivateEndpoint::open(&[]) {
        Ok(e) => e,
        Err(e) => return fail(world, e),
    };
    let workdir = std::env::temp_dir().join(format!(
        "skyfold-setup-{}-{}",
        std::process::id(),
        fastrand::u32(..)
    ));
    if let Err(e) = std::fs::create_dir_all(&workdir) {
        return fail(world, e.to_string());
    }
    let picks: Vec<&str> = plan
        .inspirations
        .iter()
        .filter_map(|id| INSPIRATIONS.iter().find(|i| i.id == id).map(|i| i.name))
        .collect();
    let mut shown = serde_json::to_value(&plan).unwrap_or(Value::Null);
    if let Some(o) = shown.as_object_mut() {
        o.remove("description");
        o.remove("seed");
    }
    let prompt = format!(
        "Their description of the world:\n\"\"\"\n{}\n\"\"\"\n\nMMOs they love: {}\n\nThe plan so far:\n{}\n",
        plan.description.trim(),
        if picks.is_empty() {
            "(none picked)".to_string()
        } else {
            picks.join(", ")
        },
        serde_json::to_string_pretty(&shown).unwrap_or_default()
    );
    let spec = agent::RunSpec {
        backend,
        model,
        effort,
        features: status
            .as_ref()
            .map(|s| s.features.clone())
            .unwrap_or_default(),
        exe: status.as_ref().and_then(|s| s.path.clone()),
        instructions: INSTRUCTIONS.to_string(),
        prompt,
        port: endpoint.port,
        api_key: key,
        messages: vec![],
        resume: None,
        timeout: Duration::from_secs(180),
        workdir: workdir.clone(),
        images: vec![],
    };
    let handle = agent::start(spec);
    let mut run = world.resource_mut::<AiRun>();
    run.handle = Some(handle);
    run.endpoint = Some(endpoint);
    run.workdir = Some(workdir);
    run.asked = Some(plan);
    let mut s = world.resource_mut::<Setup>();
    s.ai = super::AiState {
        running: true,
        status: format!("Reading your description with {}…", backend.name()),
        understood: String::new(),
        error: None,
    };
}

/// Takes the AI's answer when it comes.
pub fn poll(world: &mut World) {
    let Some(mut run) = world.get_resource_mut::<AiRun>() else {
        return;
    };
    let Some(h) = run.handle.as_ref() else { return };
    let mut events = vec![];
    if let Ok(rx) = h.rx.lock() {
        while let Ok(e) = rx.try_recv() {
            events.push(e);
        }
    }
    let mut end: Option<Result<String, String>> = None;
    let mut status = None;
    for e in events {
        match e {
            AgentEvent::Status(t) => status = Some(t),
            AgentEvent::Message(_) => {}
            AgentEvent::Finished { summary, .. } => end = Some(Ok(summary)),
            AgentEvent::Failed(e) => end = Some(Err(e)),
        }
    }
    let Some(end) = end else {
        if let Some(t) = status {
            world.resource_mut::<Setup>().ai.status = t;
        }
        return;
    };
    let asked = run.asked.take().unwrap_or_default();
    run.handle = None;
    run.endpoint = None;
    if let Some(d) = run.workdir.take() {
        let _ = std::fs::remove_dir_all(d);
    }
    let result = end.and_then(|text| merge(&asked, &text));
    let mut s = world.resource_mut::<Setup>();
    s.ai.running = false;
    match result {
        Ok((plan, understood)) => {
            // (the user's own later edits win over the answer: only what the AI changed is taken)
            let mut cur = s.plan.clone();
            if cur == asked {
                cur = plan;
            } else {
                cur.names = plan.names;
            }
            s.plan = cur;
            s.ai.understood = understood;
            s.ai.error = None;
            s.ai.status.clear();
        }
        Err(e) => {
            s.ai.error = Some(e);
            s.ai.status.clear();
        }
    }
}

/// The plan with the AI's answer applied (only known fields; bad values are ignored), and what
/// it understood.
fn merge(plan: &WorldPlan, text: &str) -> Result<(WorldPlan, String), String> {
    let (a, b) = (
        text.find('{').ok_or("the AI didn't answer with a plan")?,
        text.rfind('}').ok_or("the AI didn't answer with a plan")?,
    );
    let answer: Value = serde_json::from_str(&text[a..=b])
        .map_err(|e| format!("the AI's answer wasn't readable: {e}"))?;
    let mut v = serde_json::to_value(plan).map_err(|e| e.to_string())?;
    if let (Some(base), Some(change)) = (
        v.as_object_mut(),
        answer.get("plan").and_then(|p| p.as_object()),
    ) {
        for (k, val) in change {
            if !base.contains_key(k) || k == "description" || k == "seed" {
                continue;
            }
            // a field at a time: one bad value doesn't spoil the rest
            let mut trial = base.clone();
            if let (Some(Value::Object(old)), Value::Object(new)) = (trial.get(k).cloned(), val) {
                let mut m = old.clone();
                for (kk, vv) in new {
                    m.insert(kk.clone(), vv.clone());
                }
                trial.insert(k.clone(), Value::Object(m));
            } else {
                trial.insert(k.clone(), val.clone());
            }
            if serde_json::from_value::<WorldPlan>(Value::Object(trial.clone())).is_ok() {
                *base = trial;
            }
        }
    }
    let mut out: WorldPlan = serde_json::from_value(v).map_err(|e| e.to_string())?;
    if let Some(n) = answer.get("names")
        && let Ok(names) = serde_json::from_value::<super::plan::Names>(n.clone())
    {
        out.names = names;
        if out.name.trim().is_empty() && !out.names.world.trim().is_empty() {
            out.name = out.names.world.clone();
        }
    }
    let understood = answer
        .get("understood")
        .and_then(|u| u.as_str())
        .unwrap_or_default()
        .to_string();
    Ok((out, understood))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_answer_changes_only_good_fields_and_brings_names() {
        let plan = WorldPlan::default();
        let text = "Sure! {\"plan\": {\"realms\": 3, \"game\": \"realm_war\", \"size\": \"huge\", \"gathering\": {\"ore\": true, \"density\": 1.8}, \"nonsense\": 1}, \"names\": {\"world\": \"Vael\", \"capital\": \"Ostmere\", \"towns\": [\"Brindle\"]}, \"understood\": \"Three realms at war.\"}";
        let (p, u) = merge(&plan, text).unwrap();
        assert_eq!(p.realms, 3);
        assert_eq!(p.game, super::super::plan::GameKind::RealmWar);
        // a bad value is left as it was
        assert_eq!(p.size, plan.size);
        assert!((p.gathering.density - 1.8).abs() < 1e-6 && p.gathering.herbs);
        assert_eq!(p.names.capital, "Ostmere");
        assert_eq!(p.name, "Vael");
        assert_eq!(u, "Three realms at war.");
    }
}
