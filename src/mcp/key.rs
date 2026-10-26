//! The MCP server's access key. 127.0.0.1 is shared by every account and sandboxed app on the
//! Mac, so each request must carry `Authorization: Bearer <key>`. The key is made once and kept
//! in `worlds/.mcp_key`, readable by this user only, so agent configs keep working across
//! launches; the stdio bridge, area-prompt runs and scripts read it from there.

use std::path::Path;
use std::sync::OnceLock;

const FILE: &str = ".mcp_key";

/// This user's key (created on first use).
pub fn get() -> &'static str {
    static KEY: OnceLock<String> = OnceLock::new();
    KEY.get_or_init(|| {
        if cfg!(test) {
            return "test-only-mcp-key".into();
        }
        load_or_create(&crate::io::worlds_dir().join(FILE))
    })
}

/// The `Authorization` header value clients send.
pub fn bearer() -> String {
    format!("Bearer {}", get())
}

/// Whether an `Authorization` header carries the key (compared in constant time).
pub fn authorized(header: Option<&str>) -> bool {
    let Some(token) = header.and_then(|h| h.trim().strip_prefix("Bearer ")) else {
        return false;
    };
    let (a, b) = (token.trim().as_bytes(), get().as_bytes());
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}

fn valid(key: &str) -> bool {
    key.len() == 64 && key.bytes().all(|b| b.is_ascii_hexdigit())
}

fn load_or_create(path: &Path) -> String {
    if let Ok(s) = std::fs::read_to_string(path)
        && valid(s.trim())
    {
        restrict(path);
        return s.trim().to_string();
    }
    let mut bytes = [0u8; 32];
    if getrandom::fill(&mut bytes).is_err() {
        // no OS randomness: a per-launch key still keeps other accounts out
        bytes.iter_mut().for_each(|b| *b = fastrand::u8(..));
    }
    let key: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    let _ = std::fs::remove_file(path);
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    if let Ok(mut f) = opts.open(path) {
        let _ = std::io::Write::write_all(&mut f, key.as_bytes());
    }
    key
}

/// Owner-only, in case the file was copied or made by hand.
fn restrict(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(m) = std::fs::metadata(path)
            && m.permissions().mode() & 0o077 != 0
        {
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_exact_bearer_key_is_accepted() {
        assert!(authorized(Some("Bearer test-only-mcp-key")));
        for h in [
            None,
            Some(""),
            Some("test-only-mcp-key"),
            Some("Bearer test-only-mcp-ke"),
            Some("Bearer test-only-mcp-key2"),
            Some("Basic test-only-mcp-key"),
        ] {
            assert!(!authorized(h), "{h:?}");
        }
    }

    #[test]
    fn the_key_file_is_made_once_and_owner_only() {
        let dir = std::env::temp_dir().join(format!("wb-mcp-key-{}", fastrand::u64(..)));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(FILE);
        let first = load_or_create(&path);
        assert!(valid(&first));
        assert_eq!(load_or_create(&path), first, "kept across launches");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
            load_or_create(&path);
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600,
                "re-restricted"
            );
        }
        std::fs::write(&path, "short").unwrap();
        assert_ne!(load_or_create(&path), "short", "a damaged key is replaced");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
