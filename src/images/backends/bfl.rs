//! Black Forest Labs' own API with the user's key: `POST {base}/v1/{model}` (header `x-key`) with
//! a JSON body answers `{id, polling_url}`; GET the polling URL (same header) until `status` is
//! `Ready`, then download `result.sample` (a signed URL, valid for ten minutes; fetched without
//! the key). Other statuses: `Pending`, `Request Moderated`, `Content Moderated`, `Error`,
//! `Failed`, `Task not found`.
//!
//! FLUX.2 models take `width` / `height` (multiples of 16) and references as `input_image`,
//! `input_image_2` … `input_image_8` (base64); Kontext and 1.1 Ultra take `aspect_ratio` and one
//! reference (`input_image`, Ultra: `image_prompt`).

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::{
    Creds, GenRequest, Http, Model, ModelSpec, Output, SizeStyle, TempFile, base64_encode,
    cfg_quote, curl, http_error, pixel_size, random_seed, shrink_reference,
};

pub const FLUX2_SIZE: SizeStyle = SizeStyle::Pixels { step: 16, min: 256, max: 2048 };

/// The API's regional addresses (the global one routes and fails over by itself).
pub const BASES: [(&str, &str); 3] = [
    ("https://api.bfl.ai", "Global"),
    ("https://api.eu.bfl.ai", "EU"),
    ("https://api.us.bfl.ai", "US"),
];

pub const MODELS: &[ModelSpec] = &[
    ModelSpec {
        id: "flux-2-pro",
        name: "FLUX.2 [pro]",
        note: "Best all-rounder; up to 8 references",
        max_refs: 8,
        size: FLUX2_SIZE,
    },
    ModelSpec {
        id: "flux-2-max",
        name: "FLUX.2 [max]",
        note: "Highest quality; slower and dearer",
        max_refs: 8,
        size: FLUX2_SIZE,
    },
    ModelSpec {
        id: "flux-2-flex",
        name: "FLUX.2 [flex]",
        note: "Fine detail and lettering",
        max_refs: 8,
        size: FLUX2_SIZE,
    },
    ModelSpec {
        id: "flux-2-klein-9b",
        name: "FLUX.2 [klein] 9B",
        note: "Fast and cheap",
        max_refs: 4,
        size: FLUX2_SIZE,
    },
    ModelSpec {
        id: "flux-kontext-pro",
        name: "FLUX.1 Kontext [pro]",
        note: "Edits one picture by instruction",
        max_refs: 1,
        size: SizeStyle::Ratio,
    },
    ModelSpec {
        id: "flux-kontext-max",
        name: "FLUX.1 Kontext [max]",
        note: "Kontext, best quality",
        max_refs: 1,
        size: SizeStyle::Ratio,
    },
    ModelSpec {
        id: "flux-pro-1.1-ultra",
        name: "FLUX1.1 [pro] Ultra",
        note: "Up to 4 MP; one image prompt",
        max_refs: 1,
        size: SizeStyle::Ratio,
    },
];

pub fn valid_base(b: &str) -> bool {
    BASES.iter().any(|(u, _)| *u == b)
}

/// A URL the key may be sent to (BFL's own hosts, https).
fn bfl_host(url: &str) -> bool {
    url.strip_prefix("https://")
        .and_then(|r| r.split('/').next())
        .is_some_and(|h| h == "bfl.ai" || h.ends_with(".bfl.ai"))
        && !url.chars().any(|c| c.is_whitespace() || c == '"' || c == '\\')
}

/// The submit body for one picture (`refs`: PNG bytes of the references).
pub fn body(model: &Model, prompt: &str, aspect: &str, seed: u64, refs: &[Vec<u8>]) -> Value {
    let mut b = json!({ "prompt": prompt, "seed": seed, "output_format": "png" });
    match model.size {
        SizeStyle::Pixels { step, min, max } => {
            let (w, h) = pixel_size(aspect, step, min, max);
            b["width"] = json!(w);
            b["height"] = json!(h);
        }
        _ => b["aspect_ratio"] = json!(aspect),
    }
    let ultra = model.id.contains("ultra");
    for (i, r) in refs.iter().take(model.max_refs).enumerate() {
        let key = match (i, ultra) {
            (0, true) => "image_prompt".to_string(),
            (0, false) => "input_image".to_string(),
            (i, _) => format!("input_image_{}", i + 1),
        };
        b[key] = json!(base64_encode(r));
    }
    b
}

fn key_header(key: &str) -> String {
    format!("header = {}\n", cfg_quote(&format!("x-key: {}", key.trim())))
}

pub fn submit_config(base: &str, model: &str, key: &str, body_file: &Path) -> String {
    format!(
        "url = {}\nrequest = \"POST\"\n{}header = \"Content-Type: application/json\"\nheader = \"accept: application/json\"\ndata-binary = {}\n",
        cfg_quote(&format!("{}/v1/{}", base.trim_end_matches('/'), model.trim())),
        key_header(key),
        cfg_quote(&format!("@{}", body_file.display()))
    )
}

/// The polling URL in a submit answer.
pub fn parse_submit(h: &Http) -> Result<String, String> {
    if !(200..300).contains(&h.status) {
        return Err(http_error("Black Forest Labs", h));
    }
    let v = h.json().ok_or_else(|| format!("Black Forest Labs: unreadable answer: {}", h.snippet()))?;
    let url = v["polling_url"].as_str().ok_or("Black Forest Labs: no polling URL in the answer")?;
    if !bfl_host(url) {
        return Err("Black Forest Labs: unexpected polling address".into());
    }
    Ok(url.to_string())
}

/// What a poll says.
#[derive(Debug, PartialEq)]
pub enum Poll {
    Waiting(String),
    Ready(String),
    Failed(String),
}

pub fn parse_poll(h: &Http) -> Poll {
    if h.status == 429 || h.status >= 500 {
        return Poll::Waiting("waiting for Black Forest Labs".into());
    }
    if !(200..300).contains(&h.status) {
        return Poll::Failed(http_error("Black Forest Labs", h));
    }
    let Some(v) = h.json() else {
        return Poll::Failed(format!("Black Forest Labs: unreadable answer: {}", h.snippet()));
    };
    let status = v["status"].as_str().unwrap_or("");
    match status {
        "Ready" => match v["result"]["sample"].as_str() {
            Some(u) if u.starts_with("https://") => Poll::Ready(u.to_string()),
            _ => Poll::Failed("Black Forest Labs: ready, but no picture address".into()),
        },
        "Request Moderated" | "Content Moderated" => {
            Poll::Failed("Black Forest Labs declined this prompt or picture (moderation)".into())
        }
        "Error" | "Failed" | "Task not found" => {
            Poll::Failed(format!("Black Forest Labs: {status}"))
        }
        "" => Poll::Waiting("queued".into()),
        s => Poll::Waiting(s.to_lowercase()),
    }
}

pub fn generate(
    req: &GenRequest,
    creds: &Creds,
    work: &Path,
    say: &dyn Fn(String),
    cancel: &AtomicBool,
) -> Result<Vec<Output>, String> {
    let key = creds.key.clone().ok_or("Add your Black Forest Labs API key in Image models")?;
    let base = if valid_base(&creds.bfl_base) { creds.bfl_base.as_str() } else { BASES[0].0 };
    let refs: Vec<Vec<u8>> = req
        .references
        .iter()
        .take(req.model.max_refs)
        .map(|r| shrink_reference(r, 1536))
        .collect::<Result<_, _>>()?;
    let seed0 = req.seed.unwrap_or_else(random_seed);
    let n = req.count.max(1);
    // submit them all (they paint in parallel on BFL's side), then poll each
    let mut polls = vec![];
    for i in 0..n {
        if cancel.load(Ordering::Relaxed) {
            return Ok(vec![]);
        }
        say(format!("Sending {} of {n} to Black Forest Labs", i + 1));
        let seed = seed0 + i as u64;
        let b = body(&req.model, &req.prompt, &req.aspect, seed, &refs);
        let f = TempFile::new(work, "body", b.to_string().as_bytes())?;
        let h = curl(&submit_config(base, &req.model.id, &key, &f.0), work, 120)?;
        polls.push((parse_submit(&h)?, seed));
    }
    let mut out = vec![];
    for (i, (url, seed)) in polls.into_iter().enumerate() {
        let start = Instant::now();
        let sample = loop {
            if cancel.load(Ordering::Relaxed) {
                return Ok(out);
            }
            if start.elapsed() > Duration::from_secs(10 * 60) {
                return Err("Black Forest Labs took too long".into());
            }
            let cfg = format!("url = {}\n{}header = \"accept: application/json\"\n", cfg_quote(&url), key_header(&key));
            match parse_poll(&curl(&cfg, work, 60)?) {
                Poll::Ready(u) => break u,
                Poll::Failed(e) => return Err(e),
                Poll::Waiting(s) => say(format!("Picture {} of {n}: {s}", i + 1)),
            }
            std::thread::sleep(Duration::from_millis(1500));
        };
        say(format!("Downloading {} of {n}", i + 1));
        let h = curl(&format!("url = {}\n", cfg_quote(&sample)), work, 120)?;
        if !(200..300).contains(&h.status) || crate::images::library::sniff(&h.body).is_none() {
            return Err(format!("Black Forest Labs: couldn't download the picture (HTTP {})", h.status));
        }
        out.push(Output { bytes: h.body, seed: Some(seed) });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::images::backends::BackendKind;

    #[test]
    fn bodies() {
        let m = Model::custom(BackendKind::Bfl, "flux-2-pro");
        let b = body(&m, "castle", "3:4", 9, &[vec![1, 2, 3], vec![4, 5, 6]]);
        assert_eq!(b["seed"], 9);
        assert_eq!(b["output_format"], "png");
        assert!(b["width"].as_u64().unwrap() % 16 == 0 && b["width"].as_u64() < b["height"].as_u64());
        assert_eq!(b["input_image"], "AQID");
        assert_eq!(b["input_image_2"], "BAUG");
        assert!(b.get("aspect_ratio").is_none());
        let k = Model::custom(BackendKind::Bfl, "flux-kontext-pro");
        let b = body(&k, "make it night", "16:9", 1, &[vec![1, 2, 3], vec![9]]);
        assert_eq!(b["aspect_ratio"], "16:9");
        assert!(b.get("input_image_2").is_none());
        let u = Model::custom(BackendKind::Bfl, "flux-pro-1.1-ultra");
        assert_eq!(body(&u, "x", "1:1", 1, &[vec![1, 2, 3]])["image_prompt"], "AQID");
        let c = submit_config("https://api.bfl.ai", "flux-2-pro", "KEY1", Path::new("/tmp/b"));
        assert!(c.contains("url = \"https://api.bfl.ai/v1/flux-2-pro\""));
        assert!(c.contains("header = \"x-key: KEY1\""));
    }

    #[test]
    fn answers() {
        let h = |status, v: Value| Http { status, body: v.to_string().into_bytes() };
        assert_eq!(
            parse_submit(&h(200, json!({"id": "a1", "polling_url": "https://api.eu2.bfl.ai/v1/get_result?id=a1"}))).unwrap(),
            "https://api.eu2.bfl.ai/v1/get_result?id=a1"
        );
        // the key never goes to a host that isn't BFL's
        assert!(parse_submit(&h(200, json!({"id": "a1", "polling_url": "https://evil.example/x"}))).is_err());
        assert!(parse_submit(&h(402, json!({"detail": "Insufficient credits"}))).unwrap_err().contains("credits"));
        assert_eq!(parse_poll(&h(200, json!({"status": "Pending"}))), Poll::Waiting("pending".into()));
        assert_eq!(
            parse_poll(&h(200, json!({"status": "Ready", "result": {"sample": "https://delivery-eu1.bfl.ai/r/s.png?sig=x"}}))),
            Poll::Ready("https://delivery-eu1.bfl.ai/r/s.png?sig=x".into())
        );
        assert!(matches!(parse_poll(&h(200, json!({"status": "Content Moderated"}))), Poll::Failed(_)));
        assert!(matches!(parse_poll(&h(503, json!({}))), Poll::Waiting(_)));
    }
}
