//! Minimal HTTP/1.1 server exposing the JSON-RPC dispatcher, plus a
//! WebSocket endpoint that streams logcat events. std::net only, thread per
//! connection, no async runtime.

use crate::service::{RpcError, VmService};
use litebox_json::Json;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub struct HttpServer {
    pub addr: String,
    pub running: Arc<AtomicBool>,
}

impl HttpServer {
    pub fn serve(addr: &str, service: Arc<Mutex<VmService>>) -> std::io::Result<()> {
        let listener = TcpListener::bind(addr)?;
        let running = Arc::new(AtomicBool::new(true));
        let r2 = running.clone();
        std::thread::spawn(move || {
            while r2.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let svc = service.clone();
                        std::thread::spawn(move || {
                            let _ = handle_conn(stream, svc);
                        });
                    }
                    Err(_) => break,
                }
            }
        });
        Ok(())
    }
}

fn handle_conn(mut stream: TcpStream, service: Arc<Mutex<VmService>>) -> std::io::Result<()> {
    stream.set_read_timeout(Some(std::time::Duration::from_secs(300)))?;
    loop {
        let req = match read_request(&mut stream) {
            Ok(Some(r)) => r,
            Ok(None) => return Ok(()),
            Err(_) => return Ok(()),
        };
        // WebSocket upgrade
        if req.path == "/events" && req.is_ws_upgrade() {
            let key = req.header("sec-websocket-key").unwrap_or_default();
            let accept = ws_accept(&key);
            let resp = format!(
                "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n"
            );
            stream.write_all(resp.as_bytes())?;
            stream.flush()?;
            ws_stream_events(&mut stream, service.clone())?;
            return Ok(());
        }
        let keep_alive = req.keep_alive();
        let resp = match route(&req, &service) {
            Ok((status, body)) => {
                format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: {}\r\n\r\n{}",
                    body.len(),
                    if keep_alive { "keep-alive" } else { "close" },
                    body
                )
            }
            Err(_) => "HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string(),
        };
        stream.write_all(resp.as_bytes())?;
        stream.flush()?;
        if !keep_alive {
            return Ok(());
        }
    }
}

struct Request {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Request {
    fn header(&self, name: &str) -> Option<String> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.clone())
    }
    fn keep_alive(&self) -> bool {
        self.header("connection")
            .map(|c| !c.eq_ignore_ascii_case("close"))
            .unwrap_or(true)
    }
    fn is_ws_upgrade(&self) -> bool {
        self.header("upgrade").map(|u| u.eq_ignore_ascii_case("websocket")).unwrap_or(false)
    }
}

fn read_request(stream: &mut TcpStream) -> std::io::Result<Option<Request>> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    // read until \r\n\r\n
    let header_end = loop {
        if let Some(pos) = find_subsequence(&buf, b"\r\n\r\n") {
            break pos;
        }
        let n = stream.read(&mut tmp)?;
        if n == 0 {
            return Ok(None);
        }
        buf.extend_from_slice(&tmp[..n]);
        if buf.len() > 64 * 1024 * 1024 {
            return Ok(None);
        }
    };
    let header_text = String::from_utf8_lossy(&buf[..header_end]).into_owned();
    let mut lines = header_text.split("\r\n");
    let request_line = lines.next().unwrap_or("");
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_uppercase();
    let path = parts.next().unwrap_or("/").to_string();
    let mut headers = Vec::new();
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            headers.push((k.trim().to_string(), v.trim().to_string()));
        }
    }
    let content_length: usize = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0);
    let body_start = header_end + 4;
    while buf.len() < body_start + content_length {
        let n = stream.read(&mut tmp)?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
    }
    let body = buf[body_start..(body_start + content_length).min(buf.len())].to_vec();
    Ok(Some(Request { method, path, headers, body }))
}

fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn route(req: &Request, service: &Arc<Mutex<VmService>>) -> Result<(String, String), ()> {
    match (req.method.as_str(), req.path.as_str()) {
        ("POST", "/rpc") => {
            let body = String::from_utf8_lossy(&req.body).into_owned();
            let reply = json_rpc_call(&body, service);
            Ok(("200 OK".to_string(), reply))
        }
        ("GET", "/status") => {
            let mut svc = service.lock().map_err(|_| ())?;
            let body = rpc_to_json(&svc.dispatch("vm.status", &Json::Null))?;
            Ok(("200 OK".to_string(), body))
        }
        ("GET", "/tools") => {
            let _svc = service.lock().map_err(|_| ())?;
            let body = tool_list_json();
            Ok(("200 OK".to_string(), body))
        }
        ("POST", "/mcp") => {
            // MCP streamable-http transport: POST a JSON-RPC message, get one back
            let body = String::from_utf8_lossy(&req.body).into_owned();
            let reply = mcp_handle(&body, service);
            Ok(("200 OK".to_string(), reply))
        }
        _ => Err(()),
    }
}

fn json_rpc_error(id: Json, code: i64, msg: &str) -> String {
    Json::obj(vec![
        ("jsonrpc", Json::str("2.0")),
        ("id", id),
        (
            "error",
            Json::obj(vec![
                ("code", Json::num(code as f64)),
                ("message", Json::str(msg)),
            ]),
        ),
    ])
    .to_string()
}

fn rpc_to_json(r: &Result<Json, RpcError>) -> Result<String, ()> {
    match r {
        Ok(v) => Ok(v.to_string()),
        Err(e) => Ok(json_rpc_error(Json::Null, e.code, &e.message)),
    }
}

/// JSON-RPC 2.0 envelope handling shared by /rpc, MCP and the CLI.
pub fn json_rpc_call(body: &str, service: &Arc<Mutex<VmService>>) -> String {
    let parsed = Json::parse(body);
    let Ok(req) = parsed else {
        return json_rpc_error(Json::Null, -32700, "parse error");
    };
    let id = req.get("id").cloned().unwrap_or(Json::Null);
    let Some(method) = req.str_field("method") else {
        return json_rpc_error(id, -32600, "missing method");
    };
    let params = req.get("params").cloned().unwrap_or(Json::Null);
    // batch of named tools as one envelope
    let is_batch = method == "batch";
    if is_batch {
        let items = params.as_arr().unwrap_or(&[]);
        let mut out = Vec::new();
        for item in items {
            let m = item.str_field("method").unwrap_or_default();
            let p = item.get("params").cloned().unwrap_or(Json::Null);
            let res = match service.lock() {
                Ok(mut svc) => svc.dispatch(m, &p),
                Err(_) => Err(RpcError::internal("service lock poisoned")),
            };
            match res {
                Ok(v) => out.push(v),
                Err(e) => out.push(Json::obj(vec![("error", Json::str(e.message))])),
            }
        }
        return Json::Arr(out).to_string();
    }
    let res = match service.lock() {
        Ok(mut svc) => svc.dispatch(method, &params),
        Err(_) => Err(RpcError::internal("service lock poisoned")),
    };
    match res {
        Ok(v) => Json::obj(vec![
            ("jsonrpc", Json::str("2.0")),
            ("id", id),
            ("result", v),
        ])
        .to_string(),
        Err(e) => json_rpc_error(id, e.code, &e.message),
    }
}

// ---------- WebSocket event stream ----------

fn ws_accept(key: &str) -> String {
    const GUID: &[u8] = b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
    let mut input = key.as_bytes().to_vec();
    input.extend_from_slice(GUID);
    crate::util::base64(&crate::util::sha1(&input))
}

fn ws_stream_events(
    stream: &mut TcpStream,
    service: Arc<Mutex<VmService>>,
) -> std::io::Result<()> {
    let mut cursor: u64 = 0;
    loop {
        std::thread::sleep(std::time::Duration::from_millis(120));
        let (events, next) = {
            let Ok(svc) = service.lock() else { return Ok(()) };
            let (e, c) = svc.machine.bus.take_after(cursor, 200);
            let next = if e.is_empty() { c } else { e.last().map(|x| x.seq + 1).unwrap_or(c) };
            (e, next)
        };
        if !events.is_empty() {
            cursor = next;
            let arr = Json::Arr(events.iter().map(|e| {
                Json::obj(vec![
                    ("seq", Json::num(e.seq as f64)),
                    ("level", Json::str(e.level.as_str())),
                    ("tag", Json::str(e.tag.clone())),
                    ("msg", Json::str(e.msg.clone())),
                ])
            }).collect::<Vec<_>>());
            let payload = Json::obj(vec![("events", arr)]).to_string();
            ws_send_text(stream, &payload)?;
        }
    }
}

fn ws_send_text(stream: &mut TcpStream, text: &str) -> std::io::Result<()> {
    let payload = text.as_bytes();
    let mut frame = Vec::with_capacity(payload.len() + 10);
    frame.push(0x81); // FIN + text
    if payload.len() < 126 {
        frame.push(payload.len() as u8);
    } else if payload.len() <= 0xFFFF {
        frame.push(126);
        frame.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    } else {
        frame.push(127);
        frame.extend_from_slice(&(payload.len() as u64).to_be_bytes());
    }
    frame.extend_from_slice(payload);
    stream.write_all(&frame)?;
    stream.flush()
}

// ---------- MCP helpers ----------

pub const MCP_TOOL_LIST: &[(&str, &str, &str)] = &[
    ("vm.status", "VM state, stats, app handles", ""),
    ("vm.reset", "Reboot the VM (fresh state)", ""),
    ("pm.install", "Install an APK by path", "{path: string}"),
    ("pm.list", "List installed packages", ""),
    ("pm.uninstall", "Remove a package", "{package: string}"),
    ("app.start", "Start an app's launcher activity", "{package: string}"),
    ("app.stop", "Stop a running app", "{handle or package: string}"),
    ("app.list", "List running app handles", ""),
    ("run", "Run the VM until pause or settle", "{budget?: number}"),
    ("resume", "Resume from a pause", "{budget?: number}"),
    ("step", "Step N instructions", "{n?: number}"),
    ("pause", "Pause the VM", ""),
    ("breakpoints.set", "Set a breakpoint (method-level or pc)", "{class?: string, method?: string} or {method: number, pc: number}"),
    ("breakpoints.list", "List breakpoints", ""),
    ("breakpoints.clear", "Clear all breakpoints", ""),
    ("state.tasks", "List VM tasks/threads", ""),
    ("state.frames", "Stack frames of a task", "{task?: number}"),
    ("state.heap", "Heap statistics", ""),
    ("state.object", "Inspect an object by ref", "{ref: number}"),
    ("state.statics", "Static fields of a class", "{class: string}"),
    ("object.write", "Patch an object field", "{ref: number, field: string, value: any}"),
    ("static.write", "Patch a static field", "{class: string, field: string, value: any}"),
    ("call", "Invoke a method directly", "{class: string, method: string, sig: string, args?: any[], this?: number}"),
    ("screen.png", "Screenshot (file or base64)", "{out?: string}"),
    ("screen.info", "Framebuffer info", ""),
    ("logcat", "Read event stream since cursor", "{cursor?: number, level?: string, tag?: string}"),
    ("snapshot.save", "Snapshot the full VM state", "{path?: string}"),
    ("snapshot.load", "Restore a snapshot", "{path: string}"),
    ("api.coverage", "Implemented framework API surface", ""),
];

pub fn tool_list_json() -> String {
    let tools = MCP_TOOL_LIST
        .iter()
        .map(|(name, desc, params)| {
            Json::obj(vec![
                ("name", Json::str(*name)),
                ("description", Json::str(*desc)),
                ("params", Json::str(*params)),
            ])
        })
        .collect();
    Json::Arr(tools).to_string()
}

pub fn mcp_handle(body: &str, service: &Arc<Mutex<VmService>>) -> String {
    let Ok(req) = Json::parse(body) else {
        return json_rpc_error(Json::Null, -32700, "parse error");
    };
    let id = req.get("id").cloned().unwrap_or(Json::Null);
    let Some(method) = req.str_field("method") else {
        return json_rpc_error(id, -32600, "missing method");
    };
    let params = req.get("params").cloned().unwrap_or(Json::Null);
    match method {
        "initialize" => Json::obj(vec![
            ("jsonrpc", Json::str("2.0")),
            ("id", id),
            (
                "result",
                Json::obj(vec![
                    ("protocolVersion", Json::str("2024-11-05")),
                    (
                        "serverInfo",
                        Json::obj(vec![
                            ("name", Json::str("litebox")),
                            ("version", Json::str(env!("CARGO_PKG_VERSION"))),
                        ]),
                    ),
                    (
                        "capabilities",
                        Json::obj(vec![("tools", Json::obj(vec![]))]),
                    ),
                ]),
            ),
        ])
        .to_string(),
        "tools/list" => {
            let tools = MCP_TOOL_LIST
                .iter()
                .map(|(name, desc, params)| {
                    Json::obj(vec![
                        ("name", Json::str(*name)),
                        ("description", Json::str(*desc)),
                        (
                            "inputSchema",
                            Json::obj(vec![
                                ("type", Json::str("object")),
                                ("additionalProperties", Json::Bool(true)),
                            ]),
                        ),
                        ("_params_hint", Json::str(*params)),
                    ])
                })
                .collect();
            Json::obj(vec![
                ("jsonrpc", Json::str("2.0")),
                ("id", id),
                ("result", Json::obj(vec![("tools", Json::Arr(tools))])),
            ])
            .to_string()
        }
        "tools/call" => {
            let name = params.str_field("name").unwrap_or_default();
            let args = params.get("arguments").cloned().unwrap_or(Json::Null);
            let res = match service.lock() {
                Ok(mut svc) => svc.dispatch(name, &args),
                Err(_) => Err(RpcError::internal("service lock poisoned")),
            };
            match res {
                Ok(v) => {
                    let text = v.to_pretty();
                    Json::obj(vec![
                        ("jsonrpc", Json::str("2.0")),
                        ("id", id),
                        (
                            "result",
                            Json::obj(vec![
                                (
                                    "content",
                                    Json::Arr(vec![Json::obj(vec![
                                        ("type", Json::str("text")),
                                        ("text", Json::str(text)),
                                    ])]),
                                ),
                                ("structuredContent", v),
                            ]),
                        ),
                    ])
                    .to_string()
                }
                Err(e) => Json::obj(vec![
                    ("jsonrpc", Json::str("2.0")),
                    ("id", id),
                    (
                        "result",
                        Json::obj(vec![
                            ("isError", Json::Bool(true)),
                            (
                                "content",
                                Json::Arr(vec![Json::obj(vec![
                                    ("type", Json::str("text")),
                                    ("text", Json::str(format!("{} {}", e.code, e.message))),
                                ])]),
                            ),
                        ]),
                    ),
                ])
                .to_string()
            }
        }
        "notifications/initialized" => Json::Null.to_string(),
        _ => json_rpc_error(id, -32601, &format!("unknown MCP method: {method}")),
    }
}
