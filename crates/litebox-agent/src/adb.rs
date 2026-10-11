//! adb passthrough: drive real Android devices (the android15 redroid
//! container, hardware devices, the official emulator) through the same
//! agent surface as the DEX VM. Every call execs the adb binary directly —
//! no shell interpolation, hard timeouts, structured results.

use litebox_json::Json;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub struct AdbError(pub String);

impl std::fmt::Display for AdbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "adb: {}", self.0)
    }
}

/// Locate the adb binary: LITEBOX_ADB env, the Android SDK the host already
/// has, then PATH.
pub fn find_adb() -> Result<PathBuf, AdbError> {
    if let Ok(p) = std::env::var("LITEBOX_ADB") {
        let path = PathBuf::from(p);
        if path.is_file() {
            return Ok(path);
        }
    }
    for candidate in [
        "/opt/android-sdk/platform-tools/adb",
        "/usr/lib/android-sdk/platform-tools/adb",
        "/usr/bin/adb",
    ] {
        let path = PathBuf::from(candidate);
        if path.is_file() {
            return Ok(path);
        }
    }
    Err(AdbError("adb binary not found (set LITEBOX_ADB)".into()))
}

fn ensure_server(adb: &PathBuf) {
    let _ = Command::new(adb).arg("start-server").stdout(Stdio::null()).stderr(Stdio::null()).status();
}

/// Run adb with a hard timeout; kills the process if it overruns.
pub fn run(adb: &PathBuf, args: &[String], timeout: Duration) -> Result<(i32, String, String), AdbError> {
    ensure_server(adb);
    let mut child = Command::new(adb)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| AdbError(format!("spawn: {e}")))?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut out = Vec::new();
                let mut err = Vec::new();
                if let Some(mut o) = child.stdout.take() {
                    use std::io::Read;
                    let _ = o.read_to_end(&mut out);
                }
                if let Some(mut e) = child.stderr.take() {
                    use std::io::Read;
                    let _ = e.read_to_end(&mut err);
                }
                return Ok((
                    status.code().unwrap_or(-1),
                    String::from_utf8_lossy(&out).into_owned(),
                    String::from_utf8_lossy(&err).into_owned(),
                ));
            }
            Ok(None) => {
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(AdbError(format!(
                        "timeout after {}s: adb {}",
                        timeout.as_secs(),
                        args.join(" ")
                    )));
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(e) => return Err(AdbError(format!("wait: {e}"))),
        }
    }
}

/// First connected device, or an explicit serial.
fn serial_arg(serial: Option<&str>) -> Result<Vec<String>, AdbError> {
    Ok(match serial {
        Some(s) => vec!["-s".to_string(), s.to_string()],
        None => vec![],
    })
}

pub fn devices(adb: &PathBuf) -> Result<Json, AdbError> {
    let (code, out, err) = run(adb, &["devices".to_string()], Duration::from_secs(10))?;
    if code != 0 {
        return Err(AdbError(err));
    }
    let mut list = Vec::new();
    for line in out.lines().skip(1) {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.split_whitespace();
        let (Some(serial), Some(state)) = (parts.next(), parts.next()) else { continue };
        list.push(Json::obj(vec![
            ("serial", Json::str(serial)),
            ("state", Json::str(state)),
        ]));
    }
    Ok(Json::obj(vec![
        ("devices", Json::Arr(list)),
        ("hint", Json::str("pass {serial} to target a device; default = first connected")),
    ]))
}

pub fn shell(adb: &PathBuf, serial: Option<&str>, cmd: &str, timeout: Duration) -> Result<Json, AdbError> {
    let mut args = serial_arg(serial)?;
    args.push("shell".to_string());
    args.push(cmd.to_string());
    let (code, out, err) = run(adb, &args, timeout)?;
    Ok(Json::obj(vec![
        ("code", Json::num(code as f64)),
        ("stdout", Json::str(out)),
        ("stderr", Json::str(err)),
    ]))
}

pub fn install(adb: &PathBuf, serial: Option<&str>, path: &str, reinstall: bool) -> Result<Json, AdbError> {
    let mut args = serial_arg(serial)?;
    if reinstall {
        args.push("-r".to_string());
    }
    args.push("install".to_string());
    args.push(path.to_string());
    let (code, out, err) = run(adb, &args, Duration::from_secs(180))?;
    let success = out.contains("Success") || (code == 0 && !out.contains("Failure"));
    Ok(Json::obj(vec![
        ("success", Json::Bool(success)),
        ("code", Json::num(code as f64)),
        ("output", Json::str(format!("{}{}", out, err).trim().to_string())),
    ]))
}

pub fn uninstall(adb: &PathBuf, serial: Option<&str>, package: &str) -> Result<Json, AdbError> {
    let mut args = serial_arg(serial)?;
    args.push("uninstall".to_string());
    args.push(package.to_string());
    let (code, out, err) = run(adb, &args, Duration::from_secs(60))?;
    Ok(Json::obj(vec![
        ("success", Json::Bool(code == 0 && out.trim() == "Success")),
        ("output", Json::str(format!("{}{}", out, err).trim().to_string())),
    ]))
}

pub fn am_start(adb: &PathBuf, serial: Option<&str>, component: &str) -> Result<Json, AdbError> {
    let mut args = serial_arg(serial)?;
    args.push("shell".to_string());
    args.push(format!("am start -W -n {component}"));
    let (code, out, err) = run(adb, &args, Duration::from_secs(60))?;
    let status = out
        .lines()
        .find(|l| l.starts_with("Status:"))
        .map(|l| l.trim_start_matches("Status:").trim().to_string())
        .unwrap_or_else(|| "unknown".into());
    Ok(Json::obj(vec![
        ("status", Json::str(status)),
        ("code", Json::num(code as f64)),
        ("output", Json::str(out)),
        ("stderr", Json::str(err)),
    ]))
}

pub fn input(adb: &PathBuf, serial: Option<&str>, kind: &str, coords: &[i64]) -> Result<Json, AdbError> {
    let mut args = serial_arg(serial)?;
    args.push("shell".to_string());
    let mut cmd = format!("input {kind}");
    for c in coords {
        cmd.push(' ');
        cmd.push_str(&c.to_string());
    }
    args.push(cmd);
    let (code, out, err) = run(adb, &args, Duration::from_secs(30))?;
    Ok(Json::obj(vec![
        ("code", Json::num(code as f64)),
        ("stdout", Json::str(out)),
        ("stderr", Json::str(err)),
    ]))
}

/// screencap: PNG bytes from the device. Caller decides file vs base64.
pub fn screencap_png(adb: &PathBuf, serial: Option<&str>) -> Result<Vec<u8>, AdbError> {
    let mut args = serial_arg(serial)?;
    args.push("exec-out".to_string());
    args.push("screencap -p".to_string());
    ensure_server(adb);
    let mut child = Command::new(adb)
        .args(&args)
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| AdbError(format!("spawn: {e}")))?;
    let deadline = Instant::now() + Duration::from_secs(45);
    // adb exec-out streams binary; read to end (adb terminates when done)
    let mut out = Vec::new();
    if let Some(mut o) = child.stdout.take() {
        use std::io::Read;
        loop {
            let mut buf = [0u8; 65536];
            match o.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    out.extend_from_slice(&buf[..n]);
                    if Instant::now() > deadline {
                        let _ = child.kill();
                        return Err(AdbError("screencap timeout".into()));
                    }
                }
                Err(e) => return Err(AdbError(format!("read: {e}"))),
            }
        }
    }
    let _ = child.wait();
    if !out.starts_with(&[0x89, b'P', b'N', b'G']) {
        return Err(AdbError(format!(
            "screencap did not return PNG: {}",
            String::from_utf8_lossy(&out[..out.len().min(120)])
        )));
    }
    Ok(out)
}

pub fn logcat(adb: &PathBuf, serial: Option<&str>, tail: usize, filter: Option<&str>) -> Result<Json, AdbError> {
    let mut args = serial_arg(serial)?;
    args.push("logcat".to_string());
    args.push("-d".to_string());
    args.push(format!("-t {tail}"));
    if let Some(f) = filter {
        args.push(f.to_string());
    }
    let (code, out, err) = run(adb, &args, Duration::from_secs(30))?;
    let lines: Vec<Json> = out.lines().map(Json::str).collect();
    Ok(Json::obj(vec![
        ("code", Json::num(code as f64)),
        ("lines", Json::Arr(lines)),
        ("stderr", Json::str(err)),
    ]))
}

pub fn packages(adb: &PathBuf, serial: Option<&str>) -> Result<Json, AdbError> {
    let mut args = serial_arg(serial)?;
    args.push("shell".to_string());
    args.push("pm list packages -3".to_string());
    let (_, out, _) = run(adb, &args, Duration::from_secs(30))?;
    let list: Vec<Json> = out
        .lines()
        .filter_map(|l| l.strip_prefix("package:"))
        .map(Json::str)
        .collect();
    Ok(Json::obj(vec![("packages", Json::Arr(list))]))
}

// ---------- uiautomator XML -> flat JSON nodes ----------

#[derive(Debug)]
pub struct UiNode {
    pub depth: usize,
    pub class: String,
    pub text: String,
    pub desc: String,
    pub resource_id: String,
    pub bounds: String,
    pub x: i64,
    pub y: i64,
    pub w: i64,
    pub h: i64,
    pub clickable: bool,
}

fn decode_entities(s: &str) -> String {
    let amp: String = char::from_u32(38).unwrap().to_string();
    let mut out = s.to_string();
    // numeric references first
    for code in [10u32, 13, 9] {
        let ent = format!("{amp}#{code};");
        let ch = char::from_u32(code).unwrap().to_string();
        out = out.replace(ent.as_str(), ch.as_str());
    }
    // named references, then the ampersand escape itself last
    let pairs: [(&str, &str); 4] = [
        ("lt;", "<"),
        ("gt;", ">"),
        ("#34;", "\""),
        ("apos;", "'"),
    ];
    for (ent, rep) in pairs {
        let full = format!("{amp}{ent}");
        out = out.replace(full.as_str(), rep);
    }
    let self_ref = format!("{amp}amp;");
    out = out.replace(self_ref.as_str(), amp.as_str());
    out
}

fn attr_val(chunk: &str, name: &str) -> String {
    // find name="value" inside a <node ...> attribute chunk
    let pat = format!("{name}=\"");
    if let Some(i) = chunk.find(&pat) {
        let rest = &chunk[i + pat.len()..];
        if let Some(end) = rest.find('"') {
            return decode_entities(&rest[..end]);
        }
    }
    String::new()
}

fn parse_bounds(s: &str) -> (i64, i64, i64, i64) {
    // "[x1,y1][x2,y2]"
    let digits: Vec<i64> = s
        .split(|c: char| !c.is_ascii_digit() && c != '-')
        .filter(|t| !t.is_empty())
        .filter_map(|t| t.parse().ok())
        .collect();
    if digits.len() >= 4 {
        let (x1, y1, x2, y2) = (digits[0], digits[1], digits[2], digits[3]);
        (x1, y1, (x2 - x1).max(0), (y2 - y1).max(0))
    } else {
        (0, 0, 0, 0)
    }
}

/// Parse a uiautomator dump into a flat node list with depth.
pub fn parse_ui_xml(xml: &str) -> Vec<UiNode> {
    let mut nodes = Vec::new();
    let mut depth = 0usize;
    let bytes = xml.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i..].starts_with(b"<node") {
            // attribute chunk up to the tag close
            let mut j = i;
            while j < bytes.len() && bytes[j] != b'>' {
                j += 1;
            }
            let chunk = &xml[i..j.min(xml.len())];
            let self_closing = j > 0 && bytes[j - 1] == b'/';
            let (x, y, w, h) = parse_bounds(&attr_val(chunk, "bounds"));
            nodes.push(UiNode {
                depth,
                class: attr_val(chunk, "class"),
                text: attr_val(chunk, "text"),
                desc: attr_val(chunk, "content-desc"),
                resource_id: attr_val(chunk, "resource-id"),
                bounds: attr_val(chunk, "bounds"),
                x,
                y,
                w,
                h,
                clickable: attr_val(chunk, "clickable") == "true",
            });
            if self_closing {
                // leaf: depth unchanged
            } else {
                depth += 1;
            }
            i = j + 1;
        } else if bytes[i..].starts_with(b"</node>") {
            depth = depth.saturating_sub(1);
            i += 7;
        } else {
            i += 1;
        }
    }
    nodes
}

pub fn ui_dump(adb: &PathBuf, serial: Option<&str>) -> Result<Json, AdbError> {
    // uiautomator writes to a device file; cat it back
    let _ = shell(adb, serial, "rm -f /sdcard/litebox-ui.xml", Duration::from_secs(15));
    let dump = shell(adb, serial, "uiautomator dump /sdcard/litebox-ui.xml", Duration::from_secs(45))?;
    let ok = dump.str_field("stdout").unwrap_or("").contains("dumped");
    if !ok {
        return Err(AdbError(format!(
            "uiautomator dump failed: {}",
            dump.str_field("stdout").unwrap_or("")
        )));
    }
    let mut args = serial_arg(serial)?;
    args.push("shell".to_string());
    args.push("cat /sdcard/litebox-ui.xml".to_string());
    let (_, xml, _) = run(adb, &args, Duration::from_secs(30))?;
    let nodes = parse_ui_xml(&xml);
    let list: Vec<Json> = nodes
        .into_iter()
        .map(|n| {
            Json::obj(vec![
                ("depth", Json::num(n.depth as f64)),
                ("class", Json::str(n.class)),
                ("text", Json::str(n.text)),
                ("desc", Json::str(n.desc)),
                ("resource_id", Json::str(n.resource_id)),
                ("x", Json::num(n.x as f64)),
                ("y", Json::num(n.y as f64)),
                ("w", Json::num(n.w as f64)),
                ("h", Json::num(n.h as f64)),
                ("clickable", Json::Bool(n.clickable)),
            ])
        })
        .collect();
    Ok(Json::obj(vec![("nodes", Json::Arr(list))]))
}
