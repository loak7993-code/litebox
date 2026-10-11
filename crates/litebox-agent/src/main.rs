//! The `litebox` binary: server, MCP, CLI, and an interactive debugger REPL.
//! All verbs go through the same dispatcher as the RPC surface.

use litebox_agent::http::HttpServer;
use litebox_agent::http::MCP_TOOL_LIST;
use litebox_agent::service::VmService;
use litebox_json::Json;
use std::io::Write as _;
use std::sync::{Arc, Mutex};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = run(args);
    std::process::exit(code);
}

fn config_from_env() -> litebox_core::Config {
    let mut cfg = litebox_core::Config::default();
    if let Ok(home) = std::env::var("LITEBOX_HOME") {
        cfg.home = std::path::PathBuf::from(home);
    }
    if let Ok(seed) = std::env::var("LITEBOX_SEED") {
        if let Ok(v) = seed.trim().parse() {
            cfg.seed = v;
        }
    }
    cfg
}

fn run(args: Vec<String>) -> i32 {
    let mut cfg = config_from_env();
    let mut it = args.into_iter().peekable();
    let Some(cmd) = it.next() else {
        print_usage();
        return 0;
    };
    let mut flags: Vec<String> = Vec::new();
    let mut rest: Vec<String> = Vec::new();
    for a in it {
        if a.starts_with("--") {
            flags.push(a);
        } else {
            rest.push(a);
        }
    }
    for f in &flags {
        if f == "--home" {
            if let Ok(h) = std::env::var("LITEBOX_HOME") {
                cfg.home = h.into();
            }
        }
        if f == "--no-http" {
            cfg.http_addr = None;
        }
    }
    match cmd.as_str() {
        "version" | "--version" => {
            println!("litebox {}", env!("CARGO_PKG_VERSION"));
            0
        }
        "help" | "--help" | "-h" => {
            print_usage();
            0
        }
        "serve" => cmd_serve(&cfg, &flags, &rest),
        "mcp" => cmd_mcp(&cfg),
        "install" => cmd_simple(&cfg, &rest, "pm.install", |path| {
            Json::obj(vec![("path", Json::str(path))])
        }),
        "apps" => cmd_simple_noargs(&cfg, "pm.list"),
        "ps" => cmd_simple_noargs(&cfg, "state.tasks"),
        "status" => cmd_simple_noargs(&cfg, "vm.status"),
        "logcat" => cmd_logcat(&cfg, flags.contains(&"--follow".to_string())),
        "screen" => cmd_screen(&cfg, &rest),
        "snapshot" => cmd_snapshot(&cfg, &rest),
        "start" => cmd_simple(&cfg, &rest, "app.start", |pkg| {
            Json::obj(vec![("package", Json::str(pkg))])
        }),
        "stop" => cmd_simple(&cfg, &rest, "app.stop", |pkg| {
            Json::obj(vec![("package", Json::str(pkg))])
        }),
        "run" => cmd_simple_noargs(&cfg, "run"),
        "debug" | "repl" => cmd_repl(&cfg),
        "tools" => {
            for (name, desc, params) in MCP_TOOL_LIST {
                println!("{name:<18} {desc:<44} {params}");
            }
            0
        }
        other => {
            eprintln!("unknown command: {other}");
            print_usage();
            2
        }
    }
}

fn print_usage() {
    println!(
        "litebox — a lightweight Android app runtime for agents\n\n\
USAGE:\n  \
litebox serve [--addr HOST:PORT]     JSON-RPC + MCP + WebSocket control plane\n  \
litebox mcp                          MCP server over stdio\n  \
litebox install <apk>                Install an APK\n  \
litebox apps                         List installed packages\n  \
litebox start <package>              Start an app (launcher activity)\n  \
litebox run                          Run until pause or settle\n  \
litebox stop <package>               Stop a running app\n  \
litebox status                       VM status\n  \
litebox ps                           Task list\n  \
litebox logcat [--follow]            Event stream\n  \
litebox screen <out.png>             Screenshot\n  \
litebox snapshot save|load [path]    VM state save/restore\n  \
litebox debug                        Interactive debugger REPL\n  \
litebox tools                        List the agent tool surface\n\n\
ENV:\n  \
LITEBOX_HOME                         Data directory (default ~/.litebox)\n  \
LITEBOX_SEED                         Deterministic RNG seed"
    );
}

fn make_service(cfg: &litebox_core::Config) -> Arc<Mutex<VmService>> {
    let svc = VmService::new(cfg.clone()).expect("service");
    Arc::new(Mutex::new(svc))
}

fn cmd_serve(cfg: &litebox_core::Config, flags: &[String], rest: &[String]) -> i32 {
    // --addr's value lands in `rest` (it doesn't start with --)
    let addr = if flags.iter().any(|f| f == "--addr") {
        rest.first().cloned()
    } else {
        None
    }
    .or_else(|| cfg.http_addr.clone())
    .unwrap_or_else(|| "127.0.0.1:7830".to_string());
    let service = make_service(cfg);
    match HttpServer::serve(&addr, service.clone()) {
        Ok(()) => {
            println!("litebox {} serving on http://{addr}", env!("CARGO_PKG_VERSION"));
            println!("  POST /rpc    JSON-RPC 2.0 (all tools)");
            println!("  POST /mcp    MCP streamable-http transport");
            println!("  GET  /events WebSocket logcat stream");
            println!("  GET  /status quick status");
            println!("  GET  /tools  tool list");
            loop {
                std::thread::sleep(std::time::Duration::from_secs(3600));
            }
        }
        Err(e) => {
            eprintln!("bind {addr}: {e}");
            1
        }
    }
}

fn cmd_mcp(cfg: &litebox_core::Config) -> i32 {
    let service = make_service(cfg);
    litebox_agent::mcp::run_stdio(service);
    0
}

fn with_service<T>(cfg: &litebox_core::Config, f: impl FnOnce(&mut VmService) -> T) -> Option<T> {
    let mut svc = VmService::new(cfg.clone()).ok()?;
    Some(f(&mut svc))
}

fn cmd_simple(cfg: &litebox_core::Config, rest: &[String], method: &str, build: impl Fn(String) -> Json) -> i32 {
    let Some(arg) = rest.first() else {
        eprintln!("missing argument for {method}");
        return 2;
    };
    let params = build(arg.clone());
    with_service(cfg, |svc| {
        match svc.dispatch(method, &params) {
            Ok(v) => println!("{}", v.to_pretty()),
            Err(e) => {
                eprintln!("error {}: {}", e.code, e.message);
                std::process::exit(1);
            }
        }
    });
    0
}

fn cmd_simple_noargs(cfg: &litebox_core::Config, method: &str) -> i32 {
    with_service(cfg, |svc| {
        match svc.dispatch(method, &Json::Null) {
            Ok(v) => println!("{}", v.to_pretty()),
            Err(e) => {
                eprintln!("error {}: {}", e.code, e.message);
                std::process::exit(1);
            }
        }
    });
    0
}

fn cmd_logcat(cfg: &litebox_core::Config, follow: bool) -> i32 {
    let mut svc = match VmService::new(cfg.clone()) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let mut cursor = 0u64;
    loop {
        let params = Json::obj(vec![("cursor", Json::num(cursor as f64))]);
        match svc.dispatch("logcat", &params) {
            Ok(v) => {
                cursor = v.u32_field("cursor").unwrap_or(cursor as u32) as u64;
                if let Some(events) = v.get("events").and_then(|e| e.as_arr()) {
                    for e in events {
                        let level = e.str_field("level").unwrap_or("?");
                        let tag = e.str_field("tag").unwrap_or("?");
                        let msg = e.str_field("msg").unwrap_or("");
                        println!("{level}/{tag}: {msg}");
                    }
                }
            }
            Err(e) => {
                eprintln!("error: {}", e.message);
                return 1;
            }
        }
        if !follow {
            return 0;
        }
        std::thread::sleep(std::time::Duration::from_millis(150));
    }
}

fn cmd_screen(cfg: &litebox_core::Config, rest: &[String]) -> i32 {
    let out = rest.first().cloned().unwrap_or_else(|| "screen.png".to_string());
    let params = Json::obj(vec![("out", Json::str(out))]);
    cmd_dispatch_print(cfg, "screen.png", &params)
}

fn cmd_snapshot(cfg: &litebox_core::Config, rest: &[String]) -> i32 {
    let (verb, path) = match rest {
        [v] => (v.clone(), None),
        [v, p] => (v.clone(), Some(p.clone())),
        _ => {
            eprintln!("usage: litebox snapshot save|load [path]");
            return 2;
        }
    };
    let mut params = Vec::new();
    if let Some(p) = path {
        params.push(("path", Json::str(p)));
    }
    let method = format!("snapshot.{verb}");
    let params = Json::obj(params);
    match with_service(cfg, |svc| svc.dispatch(&method, &params)) {
        Some(Ok(v)) => {
            println!("{}", v.to_pretty());
            0
        }
        Some(Err(e)) => {
            eprintln!("error: {}", e.message);
            1
        }
        None => 1,
    }
}

fn cmd_dispatch_print(cfg: &litebox_core::Config, method: &str, params: &Json) -> i32 {
    match with_service(cfg, |svc| svc.dispatch(method, params)) {
        Some(Ok(v)) => {
            println!("{}", v.to_pretty());
            0
        }
        Some(Err(e)) => {
            eprintln!("error: {}", e.message);
            1
        }
        None => 1,
    }
}

/// Interactive debugger REPL: type tool names with JSON args.
fn cmd_repl(cfg: &litebox_core::Config) -> i32 {
    let mut svc = match VmService::new(cfg.clone()) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    println!(
        "litebox debugger — commands are the tool surface; JSON args optional.\n  try: status | apps | state.tasks | run | step {{\"n\":5}} | help"
    );
    let stdin = std::io::stdin();
    loop {
        print!("litebox> ");
        std::io::stdout().flush().ok();
        let mut line = String::new();
        if stdin.read_line(&mut line).unwrap_or(0) == 0 {
            return 0;
        }
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if line == "help" || line == "tools" {
            for (name, desc, _) in MCP_TOOL_LIST {
                println!("{name:<18} {desc}");
            }
            continue;
        }
        if line == "quit" || line == "exit" {
            return 0;
        }
        // parse: toolname {json args} or toolname bare-arg
        let (method, params) = parse_repl_line(line);
        match svc.dispatch(&method, &params) {
            Ok(v) => println!("{}", v.to_pretty()),
            Err(e) => println!("error {}: {}", e.code, e.message),
        }
    }
}

fn parse_repl_line(line: &str) -> (String, Json) {
    let mut parts = line.splitn(2, char::is_whitespace);
    let method = parts.next().unwrap_or("").to_string();
    let rest = parts.next().unwrap_or("").trim();
    let params = if rest.is_empty() {
        Json::Null
    } else if rest.starts_with('{') || rest.starts_with('[') {
        Json::parse(rest).unwrap_or(Json::str(rest))
    } else {
        // bare argument: guess the conventional field name
        let key = match method.as_str() {
            "pm.install" => "path",
            "pm.uninstall" | "app.start" | "app.stop" | "state.statics" => "package",
            "call" => "sig",
            _ => "package",
        };
        Json::obj(vec![(key, Json::str(rest))])
    };
    (method, params)
}
