//! Codex: the Codex CLI (on the PATH, or the copy inside the ChatGPT / Codex apps:
//! `crate::prompt::agent::find_codex`) with the user's ChatGPT sign-in paints with its
//! `$imagegen` skill. A job folder gets a `BRIEF.md`; `codex exec` (workspace-write sandbox in
//! that folder only, references attached with `-i`) saves `out_1.png` … there. The user's
//! `~/.codex/config.toml` is never changed: everything is passed on the command line.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use super::{GenRequest, ModelSpec, Output, SizeStyle};

pub const MODELS: &[ModelSpec] = &[ModelSpec {
    id: "imagegen",
    name: "ChatGPT images",
    note: "Your ChatGPT sign-in through Codex; no key needed",
    max_refs: 4,
    size: SizeStyle::Words,
}];

/// Codex's instruction (the brief carries the details).
pub const INSTRUCTION: &str =
    "Use the $imagegen skill for the images. Read BRIEF.md and carry it out completely.";

/// "landscape (16:9)" etc.
fn shape(aspect: &str) -> String {
    let (a, b) = super::ratio(aspect);
    let word = match a.cmp(&b) {
        std::cmp::Ordering::Greater => "landscape",
        std::cmp::Ordering::Less => "portrait",
        std::cmp::Ordering::Equal => "square",
    };
    format!("{word} ({a}:{b})")
}

/// The brief for a request (`refs`: the attached reference files' names, in order).
pub fn brief(req: &GenRequest, refs: &[String]) -> String {
    let n = req.count.clamp(1, 4);
    let files: Vec<String> = (1..=n).map(|i| format!("`out_{i}.png`")).collect();
    let mut b = format!(
        "# Images for Skyfold Studio\n\n\
         Make {n} picture{s} with the $imagegen skill from this prompt:\n\n\
         > {prompt}\n\n\
         - Shape: {shape}.\n\
         - Save {them} in this folder as {files} (PNG). Nothing else in the folder needs changing.\n\
         - {variety}\n\
         - Don't draw text, captions or borders unless the prompt asks for them.\n\
         - Don't edit or post-process the pictures with code; if one clearly fails the prompt, make it once more.\n",
        s = if n == 1 { "" } else { "s" },
        prompt = req.prompt.trim().replace('\n', "\n> "),
        shape = shape(&req.aspect),
        them = if n == 1 { "it" } else { "them" },
        files = files.join(", "),
        variety = if n == 1 {
            "One finished picture.".to_string()
        } else {
            format!("{n} distinct takes on the same prompt (vary composition and detail, keep the subject).")
        },
    );
    if !refs.is_empty() {
        b += &format!(
            "\n## Reference images\n\nAttached, in this order: {}. Use them as the prompt says \
             (edit, restyle, keep the subject); when it doesn't say, keep their subject and style.\n",
            refs.join(", ")
        );
    }
    b += "\nWhen done, reply with one line: the files you saved.\n";
    b
}

/// The `codex exec` arguments (after the binary).
pub fn args(work: &Path, refs: &[PathBuf]) -> Vec<String> {
    let mut a: Vec<String> = [
        "exec",
        "--skip-git-repo-check",
        "-s",
        "workspace-write",
        "-c",
        "model_reasoning_effort=low",
        "-C",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    a.push(work.to_string_lossy().into());
    a.push("-o".into());
    a.push(work.join("reply.txt").to_string_lossy().into());
    // (the prompt goes before `-i`: that flag takes any number of files)
    a.push(INSTRUCTION.into());
    if !refs.is_empty() {
        a.push("-i".into());
        a.extend(refs.iter().map(|r| r.to_string_lossy().to_string()));
    }
    a
}

/// PATH for the CLI (GUI apps don't get the shell's; the npm launcher needs node).
fn child_path() -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    let mut v: Vec<String> = std::env::var("PATH")
        .unwrap_or_default()
        .split(':')
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect();
    for d in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin"] {
        v.push(d.into());
    }
    for d in [".npm-global/bin", ".local/bin", ".bun/bin", ".volta/bin"] {
        v.push(format!("{home}/{d}"));
    }
    let mut seen: Vec<String> = vec![];
    for d in v {
        if !seen.contains(&d) {
            seen.push(d);
        }
    }
    seen.join(":")
}

/// Whether Codex is installed and signed in (blocking: call off the main thread).
pub fn status() -> (bool, String) {
    let Some(exe) = crate::prompt::agent::find_codex() else {
        return (false, "Codex isn't installed: install the ChatGPT app or the Codex CLI".into());
    };
    let out = Command::new(&exe)
        .args(["login", "status"])
        .env("PATH", child_path())
        .stdin(Stdio::null())
        .output();
    match out {
        Ok(o) if o.status.success() => (true, "Signed in to ChatGPT through Codex".into()),
        _ => (false, "Codex is installed but not signed in: run `codex login`".into()),
    }
}

/// The pictures Codex saved: `out_N.png` first, then any other new picture.
pub fn collect(work: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(work)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            let n = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
            ["png", "jpg", "jpeg", "webp"].contains(&ext.as_str()) && !n.starts_with("ref_")
        })
        .collect();
    v.sort_by_key(|p| {
        let n = p.file_stem().and_then(|n| n.to_str()).unwrap_or("").to_string();
        let k = n.strip_prefix("out_").and_then(|d| d.parse::<u32>().ok());
        (k.is_none(), k.unwrap_or(0), n)
    });
    v
}

pub fn generate(
    req: &GenRequest,
    work: &Path,
    say: &dyn Fn(String),
    cancel: &AtomicBool,
) -> Result<Vec<Output>, String> {
    let exe = crate::prompt::agent::find_codex()
        .ok_or("Codex isn't installed: install the ChatGPT app or the Codex CLI")?;
    // references copied in, so Codex reads them from its own folder
    let mut refs = vec![];
    for (i, r) in req.references.iter().take(4).enumerate() {
        let ext = r.extension().and_then(|e| e.to_str()).unwrap_or("png").to_lowercase();
        let p = work.join(format!("ref_{}.{ext}", i + 1));
        std::fs::copy(r, &p).map_err(|e| format!("{}: {e}", r.display()))?;
        refs.push(p);
    }
    let names: Vec<String> = refs
        .iter()
        .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
        .collect();
    std::fs::write(work.join("BRIEF.md"), brief(req, &names)).map_err(|e| e.to_string())?;
    let log = std::fs::File::create(work.join("codex.log")).map_err(|e| e.to_string())?;
    let err = log.try_clone().map_err(|e| e.to_string())?;
    let mut cmd = Command::new(&exe);
    cmd.args(args(work, &refs))
        .env("PATH", child_path())
        .stdin(Stdio::null())
        .stdout(log)
        .stderr(err);
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut child = cmd.spawn().map_err(|e| format!("couldn't start Codex: {e}"))?;
    let t0 = Instant::now();
    let kill = |pid: u32| {
        let _ = Command::new("/bin/kill").args(["-TERM", &format!("-{pid}")]).status();
    };
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if cancel.load(Ordering::Relaxed) => {
                kill(child.id());
                let _ = child.wait();
                return Ok(vec![]);
            }
            Ok(None) if t0.elapsed() > Duration::from_secs(20 * 60) => {
                kill(child.id());
                let _ = child.wait();
                return Err("Codex took too long".into());
            }
            Ok(None) => {
                let s = t0.elapsed().as_secs();
                say(format!("Codex is painting · {}:{:02}", s / 60, s % 60));
                std::thread::sleep(Duration::from_millis(1000));
            }
            Err(e) => return Err(e.to_string()),
        }
    }
    let files = collect(work);
    if files.is_empty() {
        let tail = std::fs::read_to_string(work.join("reply.txt")).unwrap_or_default();
        let tail = tail.trim().chars().take(200).collect::<String>();
        return Err(if tail.is_empty() {
            "Codex didn't save a picture (see codex.log in the job folder)".into()
        } else {
            format!("Codex didn't save a picture: {tail}")
        });
    }
    files
        .iter()
        .take(req.count.clamp(1, 4) as usize)
        .map(|f| {
            Ok(Output {
                bytes: std::fs::read(f).map_err(|e| e.to_string())?,
                seed: None,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::images::backends::{BackendKind, Model};

    #[test]
    fn brief_and_args() {
        let req = GenRequest {
            model: Model::custom(BackendKind::Codex, "imagegen"),
            prompt: "a misty fjord".into(),
            count: 2,
            aspect: "16:9".into(),
            seed: None,
            references: vec![],
        };
        let b = brief(&req, &["ref_1.png".into()]);
        assert!(b.contains("$imagegen") && b.contains("`out_1.png`, `out_2.png`"));
        assert!(b.contains("landscape (16:9)") && b.contains("ref_1.png"));
        let a = args(Path::new("/tmp/job"), &[PathBuf::from("/tmp/job/ref_1.png")]);
        assert_eq!(a[0], "exec");
        let i = a.iter().position(|x| x == "-i").unwrap();
        assert_eq!(a[i - 1], INSTRUCTION);
        assert_eq!(a[i + 1], "/tmp/job/ref_1.png");
        assert!(a.windows(2).any(|w| w[0] == "-s" && w[1] == "workspace-write"));
    }
}
