//! MCP Streamable HTTP transport (spec 2025-06-18): JSON-RPC 2.0 over `POST /mcp`, answered
//! with `application/json` (this server never needs to stream, so `GET` is 405). Sessions are
//! issued on `initialize` (`Mcp-Session-Id`); requests from other origins or hosts are refused
//! (DNS rebinding), the socket only listens on 127.0.0.1, and every request needs the
//! `Authorization: Bearer` key (`key.rs`) since other local accounts share that address.

use std::io::Read;
use std::sync::{Arc, mpsc};
use std::thread::JoinHandle;
use std::time::Duration;

use serde_json::{Value, json};
use tiny_http::{Header, Method, Request, Response, Server};

use super::{CallResult, Job, Out, Shared};

/// Protocol versions this server speaks (the newest it was written against first).
const VERSIONS: [&str; 4] = ["2025-06-18", "2025-11-25", "2025-03-26", "2024-11-05"];
/// How long a `tools/call` may wait for the main thread.
const CALL_TIMEOUT: Duration = Duration::from_secs(300);
const MAX_BODY: usize = 16 << 20;

const INSTRUCTIONS: &str = "Skyfold Studio is a 3D world editor for MMO worlds; these tools drive it like a person using its UI. \
Coordinates are metres: x east, z south, y up; (0,0) is the map centre and the world spans ±size/2 (world_info). \
Start with world_info, then terrain_sample (format \"ascii\") for a map of sea, land and mountains. \
Conventions, the same in every tool: positions are [x, z] arrays (or [x, y, z] with an exact height); paths, outlines and polygons are lists of [x, z] points. \
Every yaw, heading or direction in degrees (yaw_deg, wind_direction_deg, flow_dir, sun_azimuth…) is a compass bearing: 0 = north (−z), 90 = east (+x), 180 = south, 270 = west, clockwise seen from above. \
For the camera and the walking explorer it is where they look; for players and NPCs where they face; for objects their turn from how they were modelled; for wind and water flow where they go. \
Arguments are checked against each tool's schema (unknown keys are errors that list the valid ones). Several calls can run as one undo step with batch; add-style tools also take `items` for several at once. \
Every edit is one undo step (undo / redo) and uses the same code as the editor; \
lighting, look, weather, camera and sea level are settings outside the undo history. \
Look at your work with screenshot (it can move the camera first). Zones are gameplay regions owned by the user: create them only when asked. \
Save with world_save.";

pub(crate) struct Handle {
    server: Arc<Server>,
    thread: Option<JoinHandle<()>>,
}

impl Handle {
    pub fn stop(mut self) {
        self.server.unblock();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

pub(crate) fn start(port: u16, shared: Arc<Shared>) -> Result<Handle, String> {
    let server = Server::http(("127.0.0.1", port)).map_err(|e| {
        let s = e.to_string();
        if s.contains("in use") {
            format!("port {port} is already in use (another Skyfold Studio?)")
        } else {
            s
        }
    })?;
    let server = Arc::new(server);
    let srv = server.clone();
    let thread = std::thread::Builder::new()
        .name("mcp-http".into())
        .spawn(move || {
            for req in srv.incoming_requests() {
                let shared = shared.clone();
                // one thread per request: a slow call (screenshot) never blocks a ping
                let _ = std::thread::Builder::new()
                    .name("mcp-request".into())
                    .spawn(move || handle(req, &shared));
            }
        })
        .map_err(|e| e.to_string())?;
    Ok(Handle {
        server,
        thread: Some(thread),
    })
}

fn header<'a>(req: &'a Request, name: &'static str) -> Option<&'a str> {
    req.headers()
        .iter()
        .find(|h| h.field.equiv(name))
        .map(|h| h.value.as_str())
}

fn hdr(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("valid header")
}

/// Is `host` (no port) this machine?
fn local_host(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "[::1]" | "::1")
}

/// `Host: 127.0.0.1:7740` → "127.0.0.1"
fn host_part(h: &str) -> &str {
    if h.starts_with('[') {
        return h.split_inclusive(']').next().unwrap_or(h);
    }
    h.split(':').next().unwrap_or(h)
}

/// Browsers send an Origin: only pages served from this machine may call (DNS rebinding,
/// drive-by requests from websites). Command-line clients send none.
fn origin_ok(origin: Option<&str>) -> bool {
    let Some(o) = origin else { return true };
    let rest = o.split_once("://").map(|(_, r)| r).unwrap_or("");
    local_host(host_part(rest.split('/').next().unwrap_or("")))
}

fn respond(
    req: Request,
    status: u16,
    body: Option<Value>,
    extra: Vec<Header>,
    origin: Option<String>,
) {
    let mut resp = match body {
        Some(v) => Response::from_string(v.to_string())
            .with_header(hdr("Content-Type", "application/json")),
        None => Response::from_string(String::new()),
    }
    .with_status_code(status)
    // always a Content-Length (simple clients and the stdio bridge don't speak chunked)
    .with_chunked_threshold(usize::MAX);
    for h in extra {
        resp.add_header(h);
    }
    if let Some(o) = origin {
        resp.add_header(hdr("Access-Control-Allow-Origin", &o));
        resp.add_header(hdr("Access-Control-Expose-Headers", "Mcp-Session-Id"));
        resp.add_header(hdr("Vary", "Origin"));
    }
    let _ = req.respond(resp);
}

fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn handle(mut req: Request, shared: &Shared) {
    let origin = header(&req, "Origin").map(str::to_string);
    if !origin_ok(origin.as_deref())
        || !header(&req, "Host").is_none_or(|h| local_host(host_part(h)))
    {
        return respond(
            req,
            403,
            Some(rpc_error(
                Value::Null,
                -32600,
                "Forbidden: only local clients may use this server",
            )),
            vec![],
            None,
        );
    }
    let path = req.url().split('?').next().unwrap_or("").to_string();
    if path != "/mcp" && path != "/" {
        return respond(req, 404, None, vec![], origin);
    }
    let session = header(&req, "Mcp-Session-Id").map(str::to_string);
    if let Some(s) = &session
        && shared
            .ended
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(s)
    {
        return respond(
            req,
            404,
            Some(rpc_error(
                Value::Null,
                -32001,
                "Session ended: initialize a new one",
            )),
            vec![],
            origin,
        );
    }
    if let Some(v) = header(&req, "MCP-Protocol-Version")
        && !VERSIONS.contains(&v)
    {
        let msg = format!(
            "Unsupported MCP-Protocol-Version {v}; supported: {}",
            VERSIONS.join(", ")
        );
        return respond(
            req,
            400,
            Some(rpc_error(Value::Null, -32600, &msg)),
            vec![],
            origin,
        );
    }
    // Every request but a CORS preflight (browsers send those without credentials) needs the key:
    // other accounts and sandboxed apps on this Mac can reach 127.0.0.1 too.
    if *req.method() != Method::Options && !super::key::authorized(header(&req, "Authorization")) {
        return respond(
            req,
            401,
            Some(rpc_error(
                Value::Null,
                -32001,
                "Unauthorized: Skyfold Studio needs its MCP key. Copy the connect command again from the MCP dialog (top bar).",
            )),
            vec![hdr("WWW-Authenticate", "Bearer")],
            origin,
        );
    }
    match req.method() {
        Method::Options => {
            let extra = vec![
                hdr("Access-Control-Allow-Methods", "POST, GET, DELETE, OPTIONS"),
                hdr(
                    "Access-Control-Allow-Headers",
                    "Content-Type, Accept, Authorization, Mcp-Session-Id, MCP-Protocol-Version, Last-Event-ID",
                ),
                hdr("Access-Control-Max-Age", "600"),
            ];
            respond(req, 204, None, extra, origin)
        }
        // no server-initiated stream: every answer comes back on its POST
        Method::Get => respond(req, 405, None, vec![hdr("Allow", "POST, DELETE")], origin),
        Method::Delete => match session {
            Some(s) => {
                shared
                    .ended
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(s);
                respond(req, 200, None, vec![], origin)
            }
            None => respond(
                req,
                400,
                Some(rpc_error(Value::Null, -32600, "Missing Mcp-Session-Id")),
                vec![],
                origin,
            ),
        },
        Method::Post => {
            let mut body = Vec::new();
            if req
                .as_reader()
                .take(MAX_BODY as u64 + 1)
                .read_to_end(&mut body)
                .is_err()
                || body.len() > MAX_BODY
            {
                return respond(
                    req,
                    413,
                    Some(rpc_error(
                        Value::Null,
                        -32600,
                        "Request too large or unreadable",
                    )),
                    vec![],
                    origin,
                );
            }
            let msg: Value = match serde_json::from_slice(&body) {
                Ok(v) => v,
                Err(e) => {
                    return respond(
                        req,
                        400,
                        Some(rpc_error(Value::Null, -32700, &format!("Parse error: {e}"))),
                        vec![],
                        origin,
                    );
                }
            };
            let mut new_session = None;
            let reply = match msg {
                // (batches were dropped from the spec in 2025-06-18; older clients may still send them)
                Value::Array(list) => {
                    let out: Vec<Value> = list
                        .into_iter()
                        .filter_map(|m| process(m, shared, &mut new_session))
                        .collect();
                    (!out.is_empty()).then_some(Value::Array(out))
                }
                m => process(m, shared, &mut new_session),
            };
            let extra: Vec<Header> = new_session
                .iter()
                .map(|s| hdr("Mcp-Session-Id", s))
                .collect();
            match reply {
                Some(v) => respond(req, 200, Some(v), extra, origin),
                // notifications and responses: accepted, nothing to say
                None => respond(req, 202, None, extra, origin),
            }
        }
        _ => respond(req, 405, None, vec![hdr("Allow", "POST, DELETE")], origin),
    }
}

/// One JSON-RPC message → its response (None for notifications and client responses).
fn process(msg: Value, shared: &Shared, new_session: &mut Option<String>) -> Option<Value> {
    let Some(method) = msg.get("method").and_then(|m| m.as_str()) else {
        // responses to server requests (this server sends none) need no answer
        if msg.get("result").is_some() || msg.get("error").is_some() {
            return None;
        }
        return Some(rpc_error(
            msg.get("id").cloned().unwrap_or(Value::Null),
            -32600,
            "Invalid request",
        ));
    };
    let id = msg.get("id").cloned()?; // notifications (initialized, cancelled…) need no answer
    let params = msg.get("params").cloned().unwrap_or(json!({}));
    let result = match method {
        "initialize" => {
            let asked = params
                .get("protocolVersion")
                .and_then(|v| v.as_str())
                .unwrap_or(VERSIONS[0]);
            let version = if VERSIONS.contains(&asked) {
                asked
            } else {
                VERSIONS[0]
            };
            *new_session = Some(format!("{:032x}", fastrand::u128(..)));
            json!({
                "protocolVersion": version,
                "capabilities": { "tools": { "listChanged": false } },
                "serverInfo": { "name": "world-builder", "title": "Skyfold Studio", "version": env!("CARGO_PKG_VERSION") },
                "instructions": INSTRUCTIONS,
            })
        }
        "ping" => json!({}),
        "tools/list" => shared
            .tools
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone(),
        "tools/call" => {
            let Some(name) = params.get("name").and_then(|n| n.as_str()) else {
                return Some(rpc_error(id, -32602, "tools/call needs a tool `name`"));
            };
            let known = shared.tools.read().unwrap_or_else(|e| e.into_inner())["tools"]
                .as_array()
                .is_some_and(|l| l.iter().any(|t| t["name"] == name));
            if !known {
                return Some(rpc_error(id, -32602, &format!("Unknown tool: {name}")));
            }
            let args = match params.get("arguments") {
                None | Some(Value::Null) => json!({}),
                Some(a) => a.clone(),
            };
            call_result(call(shared, name, args))
        }
        _ => {
            return Some(rpc_error(
                id,
                -32601,
                &format!("Method not found: {method}"),
            ));
        }
    };
    Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
}

/// Queues a call for the main thread and waits for its result.
fn call(shared: &Shared, name: &str, args: Value) -> CallResult {
    let (tx, rx) = mpsc::channel();
    shared
        .queue
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push_back(Job {
            name: name.to_string(),
            args,
            reply: tx,
        });
    // an editor idling in the background wakes at once and runs at full speed while called
    crate::background::mcp_request();
    match rx.recv_timeout(CALL_TIMEOUT) {
        Ok(r) => r,
        Err(mpsc::RecvTimeoutError::Timeout) => Err(
            "The editor did not answer in time (it may be busy, or its window minimised)".into(),
        ),
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            Err("The editor dropped the call (MCP turned off or the editor is closing)".into())
        }
    }
}

fn call_result(r: CallResult) -> Value {
    match r {
        Ok(out) => {
            let content: Vec<Value> = out
                .into_iter()
                .map(|o| match o {
                    Out::Text(t) => json!({ "type": "text", "text": t }),
                    Out::Png(b) => {
                        json!({ "type": "image", "data": base64(&b), "mimeType": "image/png" })
                    }
                })
                .collect();
            let content = if content.is_empty() {
                vec![json!({ "type": "text", "text": "Done." })]
            } else {
                content
            };
            json!({ "content": content, "isError": false })
        }
        Err(e) => json!({ "content": [{ "type": "text", "text": e }], "isError": true }),
    }
}

/// Standard base64 with padding.
pub(crate) fn base64(b: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(b.len().div_ceil(3) * 4);
    for c in b.chunks(3) {
        let n = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | *c.get(2).unwrap_or(&0) as u32;
        s.push(A[(n >> 18) as usize & 63] as char);
        s.push(A[(n >> 12) as usize & 63] as char);
        s.push(if c.len() > 1 {
            A[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        s.push(if c.len() > 2 {
            A[n as usize & 63] as char
        } else {
            '='
        });
    }
    s
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpStream;

    fn raw(port: u16, method: &str, auth: Option<&str>) -> u16 {
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let auth = auth.map(|a| format!("Authorization: {a}\r\n")).unwrap_or_default();
        write!(s, "{method} /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\n{auth}Content-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
        let mut out = String::new();
        let _ = s.read_to_string(&mut out);
        out.split_whitespace().nth(1).and_then(|c| c.parse().ok()).unwrap_or(0)
    }

    #[test]
    fn requests_without_the_key_are_refused() {
        let port = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap().local_addr().unwrap().port();
        let handle = super::start(port, Default::default()).unwrap();
        assert_eq!(raw(port, "POST", None), 401, "no key (another local account)");
        assert_eq!(raw(port, "POST", Some("Bearer wrong")), 401);
        assert_eq!(raw(port, "OPTIONS", None), 204, "CORS preflights carry no credentials");
        assert_eq!(raw(port, "POST", Some(&super::super::key::bearer())), 200);
        let (status, _, body) = super::super::bridge::post(port, r#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#, None).unwrap();
        assert_eq!(status, 200, "the stdio bridge sends the key: {body}");
        handle.stop();
    }
}
