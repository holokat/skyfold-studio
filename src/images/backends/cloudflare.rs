//! Cloudflare Workers AI: `POST https://api.cloudflare.com/client/v4/accounts/{account}/ai/run/{model}`
//! with `Authorization: Bearer {token}`.
//!
//! - FLUX.2 models (`flux-2-dev`, `flux-2-klein-4b`, `flux-2-klein-9b`) take multipart form data
//!   only, even for a bare prompt: `prompt`, `width` / `height` (256–1920), `seed`, `steps`,
//!   `guidance`, and up to four reference images `input_image_0..3` (each under 512 × 512).
//! - `flux-1-schnell` takes JSON `{prompt, steps}` (its schema allows nothing else: no seed, no
//!   size; it paints 1024 × 1024 JPEGs).
//!
//! Both answer `{"result": {"image": "<base64>"}, "success": true}`; a raw image body is taken too.
//!
//! Partner models named without `@cf/` (`black-forest-labs/flux-3-image-klein`) are routed:
//! `POST …/accounts/{account}/ai/run` with JSON `{"model": id, "input": {prompt, aspect_ratio,
//! resolution, seed, images}}` (FLUX 3 Image Klein: aspect ratios 21:9 … 9:21, resolution 768sq /
//! 1k / 1.5k / 2k / 4k, up to 10 reference images as URIs: ours go as PNG data URIs). They answer
//! with the picture's URL (`result.image` or `result.sample`), downloaded without the token.
//!
//! The account id and token are always the user's own (Image models; the token in their Keychain).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{Value, json};

use super::{
    Creds, GenRequest, Http, Model, ModelSpec, Output, SizeStyle, TempFile, base64_decode,
    base64_encode, cfg_quote, curl, http_error, pixel_size, random_seed, shrink_reference,
};

pub const FLUX2_SIZE: SizeStyle = SizeStyle::Pixels { step: 16, min: 256, max: 1920 };

pub const MODELS: &[ModelSpec] = &[
    ModelSpec {
        id: "black-forest-labs/flux-3-image-klein",
        name: "FLUX 3 Image Klein",
        note: "Newest FLUX; edits and mixes up to 10 references",
        max_refs: 10,
        size: SizeStyle::Ratio,
    },
    ModelSpec {
        id: "@cf/black-forest-labs/flux-2-klein-9b",
        name: "FLUX.2 [klein] 9B",
        note: "Fast, good quality; edits with up to 4 references",
        max_refs: 4,
        size: FLUX2_SIZE,
    },
    ModelSpec {
        id: "@cf/black-forest-labs/flux-2-klein-4b",
        name: "FLUX.2 [klein] 4B",
        note: "Fastest FLUX.2; quick drafts and edits",
        max_refs: 4,
        size: FLUX2_SIZE,
    },
    ModelSpec {
        id: "@cf/black-forest-labs/flux-2-dev",
        name: "FLUX.2 [dev]",
        note: "Most detailed on Cloudflare; slower",
        max_refs: 4,
        size: FLUX2_SIZE,
    },
    ModelSpec {
        id: "@cf/black-forest-labs/flux-1-schnell",
        name: "FLUX.1 [schnell]",
        note: "Cheap square drafts; no references",
        max_refs: 0,
        size: SizeStyle::Square,
    },
];

/// The account id as Cloudflare shows it (32 hex characters; anything alphanumeric is let through).
pub fn valid_account(a: &str) -> bool {
    let a = a.trim();
    (8..=64).contains(&a.len()) && a.chars().all(|c| c.is_ascii_alphanumeric())
}

pub fn url(account: &str, model: &str) -> String {
    if routed_id(model) {
        return format!("https://api.cloudflare.com/client/v4/accounts/{}/ai/run", account.trim());
    }
    format!(
        "https://api.cloudflare.com/client/v4/accounts/{}/ai/run/{}",
        account.trim(),
        model.trim()
    )
}

/// A partner model (no `@cf/`): named in the body, its settings under `input`.
fn routed_id(id: &str) -> bool {
    !id.trim().starts_with("@cf/") && id.contains('/')
}

pub fn routed(model: &Model) -> bool {
    routed_id(&model.id)
}

/// Whether the model takes multipart form data (the FLUX.2 family).
pub fn multipart(model: &Model) -> bool {
    !routed(model) && (model.id.contains("flux-2") || model.id.contains("flux-3"))
}

/// Aspect ratios FLUX 3 accepts.
const ROUTED_ASPECTS: [&str; 15] = [
    "21:9", "2:1", "16:9", "3:2", "7:5", "4:3", "5:4", "1:1", "4:5", "3:4", "5:7", "2:3", "9:16", "1:2", "9:21",
];

/// A request body.
#[derive(Debug, PartialEq)]
pub enum Body {
    Json(Value),
    /// (field, text) pairs and (field, file) images
    Form { fields: Vec<(String, String)>, files: Vec<(String, PathBuf)> },
}

/// The body for one picture. `refs`: reference images already shrunk and written as files;
/// `uris`: the same as data URIs (routed models).
pub fn body(model: &Model, prompt: &str, aspect: &str, seed: u64, refs: &[PathBuf], uris: &[String]) -> Body {
    if routed(model) {
        let mut input = json!({ "prompt": prompt, "resolution": "1k", "seed": seed % 4_294_967_296 });
        if ROUTED_ASPECTS.contains(&aspect) {
            input["aspect_ratio"] = json!(aspect);
        }
        if !uris.is_empty() {
            input["images"] = json!(uris.iter().take(model.max_refs.min(10)).collect::<Vec<_>>());
        }
        return Body::Json(json!({ "model": model.id.trim(), "input": input }));
    }
    if !multipart(model) {
        return Body::Json(json!({ "prompt": prompt }));
    }
    let (w, h) = match model.size {
        SizeStyle::Pixels { step, min, max } => pixel_size(aspect, step, min, max),
        _ => (1024, 1024),
    };
    let fields = vec![
        ("prompt".to_string(), prompt.to_string()),
        ("width".into(), w.to_string()),
        ("height".into(), h.to_string()),
        ("seed".into(), seed.to_string()),
    ];
    let files = refs
        .iter()
        .take(model.max_refs.min(4))
        .enumerate()
        .map(|(i, p)| (format!("input_image_{i}"), p.clone()))
        .collect();
    Body::Form { fields, files }
}

/// The curl config for a request (`json_file`: where a JSON body was written).
pub fn config(account: &str, model: &str, token: &str, body: &Body, json_file: Option<&Path>) -> String {
    let mut c = format!(
        "url = {}\nrequest = \"POST\"\nheader = {}\n",
        cfg_quote(&url(account, model)),
        cfg_quote(&format!("Authorization: Bearer {}", token.trim()))
    );
    match body {
        Body::Json(_) => {
            c += "header = \"Content-Type: application/json\"\n";
            if let Some(f) = json_file {
                c += &format!("data-binary = {}\n", cfg_quote(&format!("@{}", f.display())));
            }
        }
        Body::Form { fields, files } => {
            // (`form-string`: a prompt starting with @ or < stays text)
            for (k, v) in fields {
                c += &format!("form-string = {}\n", cfg_quote(&format!("{k}={v}")));
            }
            for (k, p) in files {
                c += &format!("form = {}\n", cfg_quote(&format!("{k}=@{};type=image/png", p.display())));
            }
        }
    }
    c
}

/// The picture in an answer: its bytes, or where to download it.
#[derive(Debug, PartialEq)]
pub enum Picture {
    Bytes(Vec<u8>),
    Url(String),
}

#[cfg(test)]
pub fn parse_image(h: &Http) -> Result<Vec<u8>, String> {
    match parse_picture(h)? {
        Picture::Bytes(b) => Ok(b),
        Picture::Url(_) => Err("Cloudflare: the answer is a link, not a picture".into()),
    }
}

pub fn parse_picture(h: &Http) -> Result<Picture, String> {
    if (200..300).contains(&h.status) && crate::images::library::sniff(&h.body).is_some() {
        return Ok(Picture::Bytes(h.body.clone()));
    }
    let v = h.json();
    let ok = (200..300).contains(&h.status) && v.as_ref().is_none_or(|v| v["success"] != json!(false));
    if !ok {
        return Err(http_error("Cloudflare", h));
    }
    let v = v.ok_or_else(|| format!("Cloudflare: unreadable answer: {}", h.snippet()))?;
    let found = v["result"]["image"]
        .as_str()
        .or(v["result"]["sample"].as_str())
        .or(v["image"].as_str())
        .ok_or("Cloudflare: the answer has no image")?;
    if found.starts_with("https://") {
        return Ok(Picture::Url(found.to_string()));
    }
    let b64 = found.split_once(";base64,").map(|(_, b)| b).unwrap_or(found);
    let bytes = base64_decode(b64)?;
    if crate::images::library::sniff(&bytes).is_none() {
        return Err("Cloudflare: the image data isn't a picture".into());
    }
    Ok(Picture::Bytes(bytes))
}

pub fn generate(
    req: &GenRequest,
    creds: &Creds,
    work: &Path,
    say: &dyn Fn(String),
    cancel: &AtomicBool,
) -> Result<Vec<Output>, String> {
    let account = creds.cloudflare_account.trim();
    if !valid_account(account) {
        return Err("Add your Cloudflare account id in Image models".into());
    }
    let token = creds.key.clone().ok_or("Add your Cloudflare API token in Image models")?;
    // references: at most 512 px a side, as PNG files next to the job
    let mut refs = vec![];
    let mut uris = vec![];
    if multipart(&req.model) {
        for (i, r) in req.references.iter().take(req.model.max_refs.min(4)).enumerate() {
            let p = work.join(format!("ref_{i}.png"));
            std::fs::write(&p, shrink_reference(r, 511)?).map_err(|e| e.to_string())?;
            refs.push(p);
        }
    } else if routed(&req.model) {
        for r in req.references.iter().take(req.model.max_refs.min(10)) {
            uris.push(format!("data:image/png;base64,{}", base64_encode(&shrink_reference(r, 1024)?)));
        }
    }
    let base = req.seed.unwrap_or_else(random_seed);
    let mut out = vec![];
    for i in 0..req.count.max(1) {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        say(format!("Painting {} of {} on Cloudflare", i + 1, req.count.max(1)));
        let seed = base + i as u64;
        let b = body(&req.model, &req.prompt, &req.aspect, seed, &refs, &uris);
        let file = match &b {
            Body::Json(v) => Some(TempFile::new(work, "body", v.to_string().as_bytes())?),
            Body::Form { .. } => None,
        };
        let cfg = config(account, &req.model.id, &token, &b, file.as_ref().map(|f| f.0.as_path()));
        let h = curl(&cfg, work, 300)?;
        drop(file);
        let bytes = match parse_picture(&h)? {
            Picture::Bytes(b) => b,
            // (a signed link: fetched without the token, which only ever goes to Cloudflare's API)
            Picture::Url(u) => {
                say(format!("Downloading {} of {}", i + 1, req.count.max(1)));
                let d = curl(&format!("url = {}\n", cfg_quote(&u)), work, 120)?;
                if !(200..300).contains(&d.status) || crate::images::library::sniff(&d.body).is_none() {
                    return Err(format!("Cloudflare: couldn't download the picture (HTTP {})", d.status));
                }
                d.body
            }
        };
        let seeded = multipart(&req.model) || routed(&req.model);
        out.push(Output { bytes, seed: seeded.then_some(seed) });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::images::backends::BackendKind;

    fn model(id: &str) -> Model {
        Model::custom(BackendKind::Cloudflare, id)
    }

    #[test]
    fn flux2_is_multipart() {
        let m = model("@cf/black-forest-labs/flux-2-klein-9b");
        let refs = vec![PathBuf::from("/tmp/job/ref_0.png")];
        let b = body(&m, "a \"mossy\" rock", "16:9", 42, &refs, &[]);
        let Body::Form { fields, files } = &b else { panic!("not a form") };
        assert!(fields.contains(&("seed".into(), "42".into())));
        assert_eq!(files[0].0, "input_image_0");
        let w: u32 = fields.iter().find(|f| f.0 == "width").unwrap().1.parse().unwrap();
        let h: u32 = fields.iter().find(|f| f.0 == "height").unwrap().1.parse().unwrap();
        assert!(w > h && w <= 1920 && h >= 256 && w % 16 == 0);
        let c = config("abc123def456abc123def456abc123de", &m.id, "TOKEN_x", &b, None);
        assert!(c.contains("url = \"https://api.cloudflare.com/client/v4/accounts/abc123def456abc123def456abc123de/ai/run/@cf/black-forest-labs/flux-2-klein-9b\""));
        assert!(c.contains("header = \"Authorization: Bearer TOKEN_x\""));
        assert!(c.contains("form-string = \"prompt=a \\\"mossy\\\" rock\""));
        assert!(c.contains("form = \"input_image_0=@/tmp/job/ref_0.png;type=image/png\""));
    }

    #[test]
    fn schnell_is_json_without_extras() {
        let m = model("@cf/black-forest-labs/flux-1-schnell");
        let b = body(&m, "pine", "16:9", 7, &[PathBuf::from("/x.png")], &[]);
        assert_eq!(b, Body::Json(json!({"prompt": "pine"})));
        let c = config("abc123def456", &m.id, "T", &b, Some(Path::new("/tmp/b.json")));
        assert!(c.contains("data-binary = \"@/tmp/b.json\""));
        assert!(c.contains("Content-Type: application/json"));
    }

    #[test]
    fn flux3_is_routed_json() {
        let m = model("black-forest-labs/flux-3-image-klein");
        assert_eq!(m.max_refs, 10);
        assert!(routed(&m) && !multipart(&m));
        let uris = vec!["data:image/png;base64,AAAA".to_string()];
        let b = body(&m, "a puppy", "16:9", 42, &[], &uris);
        assert_eq!(
            b,
            Body::Json(json!({"model": "black-forest-labs/flux-3-image-klein", "input": {
                "prompt": "a puppy", "resolution": "1k", "seed": 42, "aspect_ratio": "16:9",
                "images": ["data:image/png;base64,AAAA"]}}))
        );
        let c = config("abc123def456", &m.id, "T", &b, Some(Path::new("/tmp/b.json")));
        assert!(c.contains("url = \"https://api.cloudflare.com/client/v4/accounts/abc123def456/ai/run\"\n"), "{c}");
        // its answer is a link
        let h = Http {
            status: 200,
            body: json!({"state": "Completed", "result": {"image": "https://cdn.example/x.png"}}).to_string().into_bytes(),
        };
        assert_eq!(parse_picture(&h).unwrap(), Picture::Url("https://cdn.example/x.png".into()));
        let h = Http {
            status: 200,
            body: json!({"status": "Ready", "result": {"sample": "https://cdn.example/y.png", "seed": 42}}).to_string().into_bytes(),
        };
        assert_eq!(parse_picture(&h).unwrap(), Picture::Url("https://cdn.example/y.png".into()));
        // the @cf models keep their own paths
        assert!(!routed(&model("@cf/black-forest-labs/flux-2-klein-9b")));
    }

    #[test]
    fn answers() {
        let png = [0x89, b'P', b'N', b'G', 1, 2, 3];
        let ok = Http {
            status: 200,
            body: json!({"result": {"image": base64_encode(&png)}, "success": true, "errors": []})
                .to_string()
                .into_bytes(),
        };
        assert_eq!(parse_image(&ok).unwrap(), png);
        let raw = Http { status: 200, body: png.to_vec() };
        assert_eq!(parse_image(&raw).unwrap(), png);
        let bad = Http {
            status: 403,
            body: json!({"success": false, "errors": [{"code": 10000, "message": "Authentication error"}]})
                .to_string()
                .into_bytes(),
        };
        let e = parse_image(&bad).unwrap_err();
        assert!(e.contains("refused") && e.contains("Authentication error"), "{e}");
        assert!(valid_account("0123456789abcdef0123456789abcdef"));
        assert!(!valid_account("abc/../x"));
    }
}
