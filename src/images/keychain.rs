//! Image-model API keys in the macOS Keychain (generic passwords under one service), through
//! `/usr/bin/security`. A key is written through the tool's stdin (`security -i`), so it never
//! shows on a command line; it's read only on the worker thread that sends a request. Automated
//! runs (`WB_SHOT` / `WB_FRESH`) never touch the Keychain.

use std::io::Write;
use std::process::{Command, Stdio};

const SERVICE: &str = "Skyfold Studio image models";
const SECURITY: &str = "/usr/bin/security";

fn test_run() -> bool {
    std::env::var("WB_SHOT").is_ok() || std::env::var("WB_FRESH").is_ok()
}

/// A key the Keychain tool can be handed inside quotes: printable, no spaces, quotes or backslashes.
pub fn valid_key(key: &str) -> bool {
    let k = key.trim();
    !k.is_empty()
        && k.len() <= 512
        && k.chars().all(|c| c.is_ascii_graphic() && c != '"' && c != '\'' && c != '\\')
}

/// Whether a key is stored for `account` (doesn't read the key).
pub fn has(account: &str) -> bool {
    !test_run()
        && Command::new(SECURITY)
            .args(["find-generic-password", "-s", SERVICE, "-a", account])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
}

/// The key stored for `account`.
pub fn get(account: &str) -> Option<String> {
    if test_run() {
        return None;
    }
    let out = Command::new(SECURITY)
        .args(["find-generic-password", "-s", SERVICE, "-a", account, "-w"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let k = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!k.is_empty()).then_some(k)
}

/// Stores (or replaces) the key for `account`.
pub fn set(account: &str, key: &str) -> Result<(), String> {
    if test_run() {
        return Err("not in test runs".into());
    }
    let key = key.trim();
    if !valid_key(key) {
        return Err("that key has spaces or quotes in it".into());
    }
    let mut c = Command::new(SECURITY)
        .arg("-i")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Keychain: {e}"))?;
    if let Some(mut si) = c.stdin.take() {
        let line = format!(
            "add-generic-password -U -s \"{SERVICE}\" -a \"{account}\" -l \"{SERVICE}\" -w \"{key}\"\n"
        );
        let _ = si.write_all(line.as_bytes());
    }
    let out = c.wait_with_output().map_err(|e| e.to_string())?;
    let err = String::from_utf8_lossy(&out.stderr);
    if out.status.success() && !err.contains("error") {
        Ok(())
    } else {
        Err("couldn't store the key in the Keychain".into())
    }
}

/// Forgets the key for `account`.
pub fn delete(account: &str) {
    if test_run() {
        return;
    }
    let _ = Command::new(SECURITY)
        .args(["delete-generic-password", "-s", SERVICE, "-a", account])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(test)]
mod tests {
    #[test]
    fn keys() {
        assert!(super::valid_key("cf_AbC-123_xyz"));
        assert!(!super::valid_key("has space"));
        assert!(!super::valid_key("q\"uote"));
        assert!(!super::valid_key(""));
    }
}
