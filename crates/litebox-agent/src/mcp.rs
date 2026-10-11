//! MCP over stdio: newline-delimited JSON-RPC on stdin/stdout, so any agent
//! (Claude, etc.) can add LiteBox as a tool server.

use crate::http::mcp_handle;
use crate::service::VmService;
use std::io::{BufRead, Write};
use std::sync::{Arc, Mutex};

pub fn run_stdio(service: Arc<Mutex<VmService>>) {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let reply = mcp_handle(&line, &service);
        if reply != "null" {
            writeln!(stdout, "{reply}").ok();
            stdout.flush().ok();
        }
    }
}
