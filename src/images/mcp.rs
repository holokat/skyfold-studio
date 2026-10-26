//! MCP tools for Images: the same code paths as the Images window.

use std::path::PathBuf;

use bevy::prelude::*;
use serde_json::{Value, json};

use super::backends::{self, BackendKind};
use super::jobs::Stage;
use super::library::ImageMeta;
use super::view::ImageView;
use super::{GenerateArgs, Images};
use crate::mcp_tools::{McpAppExt, McpContent, McpTool, arg, opt_arg};

pub fn register(app: &mut App) {
    app.mcp_tool(McpTool {
        name: "images_list",
        description: "The image library (generated and imported pictures kept with the user's notes): id, name, file path, prompt, backend, model, seed, size, notes, tags, favourite, parent (the picture it was varied / edited from), provenance. Newest first. Optional `query` (matches name, prompt, notes, tags), `tag`, `favourites` (only those), `source` (generated / imported), `limit`. Also lists running and recent generation jobs.",
        input_schema: || json!({"type": "object", "properties": {
            "query": {"type": "string"}, "tag": {"type": "string"}, "favourites": {"type": "boolean"},
            "source": {"type": "string", "enum": ["generated", "imported"]}, "limit": {"type": "integer"}}}),
        run: list,
    })
    .mcp_tool(McpTool {
        name: "images_get",
        description: "One library picture's details; with `image: true` also the picture itself (PNG, at most `max_size` px a side, default 768) to look at.",
        input_schema: || json!({"type": "object", "properties": {"id": {"type": "string"},
            "image": {"type": "boolean"}, "max_size": {"type": "integer"}}, "required": ["id"]}),
        run: get,
    })
    .mcp_tool(McpTool {
        name: "images_set",
        description: "Change a library picture: `name`, `notes` (replaces them; free text), `tags` (replaces them), `add_tags` / `remove_tags`, `favourite`. Returns the picture.",
        input_schema: || json!({"type": "object", "properties": {"id": {"type": "string"},
            "name": {"type": "string"}, "notes": {"type": "string"},
            "tags": {"type": "array", "items": {"type": "string"}},
            "add_tags": {"type": "array", "items": {"type": "string"}},
            "remove_tags": {"type": "array", "items": {"type": "string"}},
            "favourite": {"type": "boolean"}}, "required": ["id"]}),
        run: set,
    })
    .mcp_tool(McpTool {
        name: "images_generate",
        description: "Generate pictures into the library in the background. `prompt`; `model` as backend:model from images_backends (default: the user's chosen model; e.g. codex:imagegen, cloudflare:@cf/black-forest-labs/flux-2-klein-9b, bfl:flux-2-pro); `count` 1–4; `aspect` (1:1, 4:3, 3:4, 16:9, 9:16); `seed` (Cloudflare FLUX.2 / BFL); `references`: library image ids (or absolute file paths) handed to the model (edit / restyle / keep the subject); `parent`: the picture this is a variation or edit of. Returns the job id at once; `wait: true` waits until it finishes (Codex takes a minute or more) and returns the new pictures. Paid backends spend the user's credits.",
        input_schema: || json!({"type": "object", "properties": {
            "prompt": {"type": "string"}, "model": {"type": "string"},
            "count": {"type": "integer", "minimum": 1, "maximum": 4},
            "aspect": {"type": "string", "enum": backends::ASPECTS},
            "seed": {"type": "integer"},
            "references": {"type": "array", "items": {"type": "string"}},
            "parent": {"type": "string"}, "wait": {"type": "boolean"}}, "required": ["prompt"]}),
        run: generate,
    })
    .mcp_tool(McpTool {
        name: "images_jobs",
        description: "Generation jobs: id, prompt, model, state (running / done / failed / stopped), progress note, the picture ids made. `stop`: a job id to stop.",
        input_schema: || json!({"type": "object", "properties": {"stop": {"type": "integer"}}}),
        run: jobs,
    })
    .mcp_tool(McpTool {
        name: "images_import",
        description: "Copy picture files (PNG, JPEG, WebP; absolute paths) into the image library. Returns the new pictures.",
        input_schema: || json!({"type": "object", "properties": {"paths": {"type": "array", "items": {"type": "string"}}}, "required": ["paths"]}),
        run: import,
    })
    .mcp_tool(McpTool {
        name: "images_duplicate",
        description: "Copy a library picture (same file, notes and tags; \" copy\" on its name). Returns the copy.",
        input_schema: || json!({"type": "object", "properties": {"id": {"type": "string"}}, "required": ["id"]}),
        run: duplicate,
    })
    .mcp_tool(McpTool {
        name: "images_delete",
        description: "Remove a picture from the library (its folder goes to the Trash).",
        input_schema: || json!({"type": "object", "properties": {"id": {"type": "string"}}, "required": ["id"]}),
        run: delete,
    })
    .mcp_tool(McpTool {
        name: "images_view",
        description: "Open the Images window, optionally with a picture picked (`id`) and the Generate box filled (`prompt`, `references` as library ids). Take a screenshot to see it.",
        input_schema: || json!({"type": "object", "properties": {"id": {"type": "string"}, "prompt": {"type": "string"},
            "references": {"type": "array", "items": {"type": "string"}}}}),
        run: view,
    })
    .mcp_tool(McpTool {
        name: "images_backends",
        description: "The image backends (codex: ChatGPT sign-in through the Codex CLI; cloudflare: Workers AI FLUX on the user's account; bfl: Black Forest Labs API with the user's key): whether each is set up and why not, and every model (key backend:model, name, note, references it takes). Keys are entered by the user in the Images window only (never through MCP). Optional: `default_model` (backend:model) to choose the default, `cloudflare_account_id`, `bfl_region` (global / eu / us), `add_model` / `remove_model` (backend:model, for a model id the built-in list lacks), `recheck`.",
        input_schema: || json!({"type": "object", "properties": {
            "default_model": {"type": "string"}, "cloudflare_account_id": {"type": "string"},
            "bfl_region": {"type": "string", "enum": ["global", "eu", "us"]},
            "add_model": {"type": "string"}, "remove_model": {"type": "string"}, "recheck": {"type": "boolean"}}}),
        run: backends_tool,
    });
}

fn images(world: &mut World) -> Mut<'_, Images> {
    let mut i = world.resource_mut::<Images>();
    i.ensure();
    i
}

fn meta_json(m: &ImageMeta) -> Value {
    let mut v = serde_json::to_value(m).unwrap_or_default();
    v["path"] = json!(super::library::Library::dir(&m.id).join(&m.file));
    v["created_date"] = json!(super::library::date(m.created));
    if m.generated() {
        v["model_name"] = json!(super::view::model_name(&m.backend, &m.model));
    }
    v
}

fn job_json(j: &super::jobs::Job) -> Value {
    json!({"id": j.id, "prompt": j.prompt, "model": j.model, "count": j.count,
        "state": j.stage.id(), "note": j.note, "images": j.images, "started": j.started})
}

/// The edit buffers of the open inspector are reloaded after an MCP change.
fn refresh_view(world: &mut World) {
    world.resource_mut::<ImageView>().buf_id = None;
}

fn list(world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    let query: String = opt_arg(&args, "query")?.unwrap_or_default();
    let tag: Option<String> = opt_arg::<String>(&args, "tag")?.map(|t| t.trim().to_lowercase());
    let favs = opt_arg::<bool>(&args, "favourites")?.unwrap_or(false);
    let source: Option<String> = opt_arg(&args, "source")?;
    let limit = opt_arg::<usize>(&args, "limit")?.unwrap_or(200);
    let i = images(world);
    let entries: Vec<Value> = i
        .lib
        .entries
        .iter()
        .filter(|e| !favs || e.favourite)
        .filter(|e| tag.as_ref().is_none_or(|t| e.tags.contains(t)))
        .filter(|e| source.as_ref().is_none_or(|s| e.source == *s))
        .filter(|e| e.matches(&query))
        .take(limit)
        .map(meta_json)
        .collect();
    let jobs: Vec<Value> = i.jobs.list.iter().map(job_json).collect();
    Ok(vec![McpContent::Json(json!({
        "count": entries.len(), "total": i.lib.entries.len(), "images": entries,
        "tags": i.lib.tags(), "jobs": jobs, "folder": super::library::root(),
    }))])
}

fn get(world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    let id: String = arg(&args, "id")?;
    let want = opt_arg::<bool>(&args, "image")?.unwrap_or(false);
    let max = opt_arg::<u32>(&args, "max_size")?.unwrap_or(768).clamp(64, 2048);
    let i = images(world);
    let m = i.lib.get(&id).cloned().ok_or_else(|| format!("no image {id}"))?;
    let mut out = vec![McpContent::Json(meta_json(&m))];
    if want {
        let path = super::library::Library::dir(&m.id).join(&m.file);
        let img = image::open(&path).map_err(|e| e.to_string())?;
        let img = if img.width() > max || img.height() > max {
            img.resize(max, max, image::imageops::FilterType::Triangle)
        } else {
            img
        };
        let mut png = std::io::Cursor::new(Vec::new());
        img.to_rgb8().write_to(&mut png, image::ImageFormat::Png).map_err(|e| e.to_string())?;
        out.push(McpContent::Png(png.into_inner()));
    }
    Ok(out)
}

fn set(world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    let id: String = arg(&args, "id")?;
    let name: Option<String> = opt_arg(&args, "name")?;
    let notes: Option<String> = opt_arg(&args, "notes")?;
    let tags: Option<Vec<String>> = opt_arg(&args, "tags")?;
    let add: Vec<String> = opt_arg(&args, "add_tags")?.unwrap_or_default();
    let remove: Vec<String> = opt_arg(&args, "remove_tags")?.unwrap_or_default();
    let fav: Option<bool> = opt_arg(&args, "favourite")?;
    if name.as_ref().is_some_and(|n| n.trim().is_empty()) {
        return Err("a name can't be empty".into());
    }
    let m = images(world).lib.update(&id, |e| {
        if let Some(n) = name {
            e.name = n.trim().to_string();
        }
        if let Some(n) = notes {
            e.notes = n;
        }
        if let Some(t) = tags {
            e.tags = t;
        }
        e.tags.extend(add);
        let remove: Vec<String> = remove.iter().map(|t| t.trim().to_lowercase()).collect();
        e.tags.retain(|t| !remove.contains(&t.to_lowercase()));
        if let Some(f) = fav {
            e.favourite = f;
        }
    })?;
    refresh_view(world);
    Ok(vec![McpContent::Json(meta_json(&m))])
}

fn generate(world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    let prompt: String = arg(&args, "prompt")?;
    let model: Option<String> = opt_arg(&args, "model")?;
    let wait = opt_arg::<bool>(&args, "wait")?.unwrap_or(false);
    let refs_in: Vec<String> = opt_arg(&args, "references")?.unwrap_or_default();
    let mut i = images(world);
    let count = opt_arg::<u32>(&args, "count")?.unwrap_or(i.settings.count);
    let aspect = opt_arg::<String>(&args, "aspect")?.unwrap_or_else(|| i.settings.aspect.clone());
    let (mut refs, mut ref_files) = (vec![], vec![]);
    for r in refs_in {
        if i.lib.get(&r).is_some() {
            refs.push(r);
        } else if r.starts_with('/') {
            ref_files.push(PathBuf::from(r));
        } else {
            return Err(format!("no image {r} (give a library id or an absolute path)"));
        }
    }
    let parent: Option<String> = opt_arg(&args, "parent")?;
    if let Some(p) = &parent
        && i.lib.get(p).is_none()
    {
        return Err(format!("no image {p}"));
    }
    let job = i.generate(GenerateArgs {
        prompt,
        model,
        count,
        aspect,
        seed: opt_arg(&args, "seed")?,
        refs,
        ref_files,
        parent,
    })?;
    if !wait {
        let j = i.jobs.get(job).map(job_json).unwrap_or_default();
        return Ok(vec![McpContent::Json(json!({"job": j, "note": "running in the background: images_jobs shows progress; images_list the results"}))]);
    }
    Ok(vec![McpContent::deferred(move |world: &mut World| {
        let i = world.resource::<Images>();
        let j = i.jobs.get(job)?;
        if j.stage == Stage::Running {
            return None;
        }
        if j.stage == Stage::Failed {
            return Some(Err(j.note.clone()));
        }
        let made: Vec<Value> = j.images.iter().filter_map(|id| i.lib.get(id)).map(meta_json).collect();
        Some(Ok(vec![McpContent::Json(json!({"job": job_json(j), "images": made}))]))
    })])
}

fn jobs(world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    let stop: Option<u64> = opt_arg(&args, "stop")?;
    let i = images(world);
    if let Some(id) = stop {
        let j = i.jobs.get(id).ok_or_else(|| format!("no job {id}"))?;
        j.stop();
    }
    let v: Vec<Value> = i.jobs.list.iter().map(job_json).collect();
    Ok(vec![McpContent::Json(json!({"jobs": v}))])
}

fn import(world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    let paths: Vec<PathBuf> = arg::<Vec<String>>(&args, "paths")?.into_iter().map(PathBuf::from).collect();
    if paths.iter().any(|p| !p.is_absolute()) {
        return Err("give absolute paths".into());
    }
    let ms = images(world).lib.import(&paths)?;
    Ok(vec![McpContent::Json(json!({"images": ms.iter().map(meta_json).collect::<Vec<_>>()}))])
}

fn duplicate(world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    let id: String = arg(&args, "id")?;
    let m = images(world).lib.duplicate(&id)?;
    Ok(vec![McpContent::Json(meta_json(&m))])
}

fn delete(world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    let id: String = arg(&args, "id")?;
    images(world).lib.delete(&id)?;
    let mut v = world.resource_mut::<ImageView>();
    if v.selected.as_deref() == Some(id.as_str()) {
        v.select(None);
    }
    v.refs.retain(|r| *r != id);
    Ok(vec![McpContent::Text(format!("deleted {id}"))])
}

fn view(world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    let id: Option<String> = opt_arg(&args, "id")?;
    let prompt: Option<String> = opt_arg(&args, "prompt")?;
    let refs: Option<Vec<String>> = opt_arg(&args, "references")?;
    {
        let i = images(world);
        for r in id.iter().chain(refs.iter().flatten()) {
            if i.lib.get(r).is_none() {
                return Err(format!("no image {r}"));
            }
        }
    }
    world.resource_mut::<crate::editor::Editor>().tab = crate::editor::Tab::Images;
    let mut v = world.resource_mut::<ImageView>();
    if id.is_some() {
        v.select(id);
    }
    if let Some(p) = prompt {
        v.prompt = p;
    }
    if let Some(r) = refs {
        v.refs = r;
    }
    Ok(vec![McpContent::Text("Images is open".into())])
}

fn backends_tool(world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    let default_model: Option<String> = opt_arg(&args, "default_model")?;
    let account: Option<String> = opt_arg(&args, "cloudflare_account_id")?;
    let region: Option<String> = opt_arg(&args, "bfl_region")?;
    let add: Option<String> = opt_arg(&args, "add_model")?;
    let remove: Option<String> = opt_arg(&args, "remove_model")?;
    let recheck = opt_arg::<bool>(&args, "recheck")?.unwrap_or(false);
    let mut i = images(world);
    let mut changed = false;
    let mut check = recheck;
    if let Some(k) = add {
        let (b, id) = backends::parse_key(&k).ok_or("add_model: use backend:model")?;
        if b == BackendKind::Codex || !backends::valid_model_id(id) {
            return Err("add_model: a cloudflare: or bfl: model id without spaces or quotes".into());
        }
        if !i.settings.custom.contains(&k) {
            i.settings.custom.push(k);
            changed = true;
        }
    }
    if let Some(k) = remove {
        i.settings.custom.retain(|c| *c != k);
        changed = true;
    }
    if let Some(k) = default_model {
        let m = i.model(&k).ok_or_else(|| format!("unknown model {k}"))?;
        i.settings.model = m.key();
        changed = true;
    }
    if let Some(a) = account {
        let a = a.trim().to_string();
        if !a.is_empty() && !backends::cloudflare::valid_account(&a) {
            return Err("that doesn't look like a Cloudflare account id".into());
        }
        i.settings.cloudflare_account = a;
        changed = true;
        check = true;
    }
    if let Some(r) = region {
        let base = match r.as_str() {
            "eu" => backends::bfl::BASES[1].0,
            "us" => backends::bfl::BASES[2].0,
            _ => backends::bfl::BASES[0].0,
        };
        i.settings.bfl_base = base.into();
        changed = true;
    }
    if changed {
        i.settings.save();
    }
    if check {
        i.check_backends();
    }
    let checking = i.checking();
    let status: Vec<Value> = BackendKind::ALL
        .iter()
        .map(|b| {
            let st = i.status.get(b);
            json!({"id": b.id(), "name": b.name(),
                "ready": st.map(|s| s.ready), "note": st.map(|s| s.note.clone()),
                "key_stored": st.map(|s| s.has_key), "keys_at": b.keys_at()})
        })
        .collect();
    let models: Vec<Value> = i
        .models()
        .iter()
        .map(|m| json!({"key": m.key(), "name": m.name, "note": m.note, "max_references": m.max_refs,
            "square_only": m.size == backends::SizeStyle::Square}))
        .collect();
    Ok(vec![McpContent::Json(json!({
        "default_model": i.settings.model, "backends": status, "models": models,
        "cloudflare_account_id": i.settings.cloudflare_account, "bfl_base": i.settings.bfl_base,
        "checking": checking,
    }))])
}
