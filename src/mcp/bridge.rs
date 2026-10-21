//! stdio bridge for MCP clients that only launch servers as processes (Claude Desktop…):
//! `world_builder --mcp-stdio [--port 7740] [--no-launch]` reads newline-delimited JSON-RPC
//! from stdin, posts each message to the running editor's HTTP endpoint and writes the answers
//! to stdout (requests run concurrently, so a slow screenshot never holds up a ping). If the
//! editor isn't running it starts it (same executable) and waits for it. Nothing but protocol
//! messages goes to stdout; notes go to stderr.

use std::io::{BufRead, Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// How long a started editor gets to open its MCP port.
const LAUNCH_WAIT: Duration = Duration::from_secs(120);

struct Bridge {
    port: u16,
    session: Mutex<Option<String>>,
    out: Mutex<std::io::Stdout>,
}

pub fn run() {
    let args: Vec<String> = std::env::args().collect();
    let port = args
        .iter()
        .position(|a| a == "--port")
        .and_then(|i| args.get(i + 1))
        .and_then(|p| p.parse::<u16>().ok())
        .or_else(|| {
            std::env::var("WB_MCP_PORT")
                .ok()
                .and_then(|p| p.parse().ok())
        })
        .or_else(|| crate::prefs::get_u64("mcp_port").map(|p| p as u16))
        .unwrap_or(super::DEFAULT_PORT);
    let mut may_launch = !args.iter().any(|a| a == "--no-launch");
    let b = Arc::new(Bridge {
        port,
        session: Mutex::new(None),
        out: Mutex::new(std::io::stdout()),
    });
    let mut workers = vec![];
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }
        if may_launch && TcpStream::connect(("127.0.0.1", port)).is_err() {
            may_launch = false;
            eprintln!(
                "world-builder bridge: the editor isn't running on port {port}; starting it…"
            );
            launch(port);
        }
        let msg: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
        let concurrent = msg.get("id").is_some()
            && msg.get("method").and_then(|m| m.as_str()) != Some("initialize");
        if concurrent {
            let b = b.clone();
            workers.push(std::thread::spawn(move || forward(&b, &line, &msg)));
        } else {
            forward(&b, &line, &msg);
        }
        workers.retain(|w| !w.is_finished());
    }
    for w in workers {
        let _ = w.join();
    }
}

/// Posts one message and writes the answer (or a JSON-RPC error if the editor is unreachable).
fn forward(b: &Bridge, line: &str, msg: &Value) {
    let session = b.session.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let mut answer = post(b.port, line, session.as_deref());
    if let Ok((404, _, _)) = answer
        && session.is_some()
    {
        // the session was ended: carry on without it
        *b.session.lock().unwrap_or_else(|e| e.into_inner()) = None;
        answer = post(b.port, line, None);
    }
    let text = match answer {
        Ok((_, sid, body)) => {
            if sid.is_some() {
                *b.session.lock().unwrap_or_else(|e| e.into_inner()) = sid;
            }
            body.trim().to_string()
        }
        Err(e) => {
            eprintln!("world-builder bridge: {e}");
            match msg.get("id") {
                Some(id) => {
                    let m = format!(
                        "Skyfold Studio is not reachable on http://127.0.0.1:{}/mcp ({e}). Start the editor and make sure MCP is on (top bar).",
                        b.port
                    );
                    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32000, "message": m } })
                        .to_string()
                }
                None => String::new(),
            }
        }
    };
    if !text.is_empty() {
        let mut out = b.out.lock().unwrap_or_else(|e| e.into_inner());
        let _ = writeln!(out, "{text}");
        let _ = out.flush();
    }
}

/// Starts the editor (this executable without `--mcp-stdio`) and waits until it listens.
fn launch(port: u16) {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let spawned = std::process::Command::new(exe)
        .env("WB_MCP_PORT", port.to_string())
        .env("WB_MCP", "1")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    if let Err(e) = spawned {
        eprintln!("world-builder bridge: could not start the editor: {e}");
        return;
    }
    let t0 = Instant::now();
    while t0.elapsed() < LAUNCH_WAIT {
        std::thread::sleep(Duration::from_millis(500));
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return;
        }
    }
}

/// POST one message; returns (status, Mcp-Session-Id, body).
pub(crate) fn post(
    port: u16,
    body: &str,
    session: Option<&str>,
) -> std::io::Result<(u16, Option<String>, String)> {
    let mut s = TcpStream::connect(("127.0.0.1", port))?;
    s.set_read_timeout(Some(Duration::from_secs(330)))?;
    let mut head = format!(
        "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    if let Some(sid) = session {
        head += &format!("Mcp-Session-Id: {sid}\r\n");
    }
    head += "\r\n";
    s.write_all(head.as_bytes())?;
    s.write_all(body.as_bytes())?;
    // headers
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    while !buf.ends_with(b"\r\n\r\n") {
        if s.read(&mut byte)? == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "connection closed",
            ));
        }
        buf.push(byte[0]);
    }
    let head = String::from_utf8_lossy(&buf).to_string();
    let mut lines = head.split("\r\n");
    let status = lines
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|c| c.parse::<u16>().ok())
        .unwrap_or(0);
    let (mut len, mut sid) = (None, None);
    for l in lines {
        if let Some((k, v)) = l.split_once(':') {
            match k.trim().to_ascii_lowercase().as_str() {
                "content-length" => len = v.trim().parse::<usize>().ok(),
                "mcp-session-id" => sid = Some(v.trim().to_string()),
                _ => {}
            }
        }
    }
    let mut body = Vec::new();
    match len {
        Some(n) => {
            body.resize(n, 0);
            s.read_exact(&mut body)?;
        }
        None => {
            s.read_to_end(&mut body)?;
        }
    }
    Ok((status, sid, String::from_utf8_lossy(&body).to_string()))
}
