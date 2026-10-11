//! Debugger state: breakpoints, pause policy, step counters, and pause
//! reasons. The control plane drives everything through `Machine`.

use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq)]
pub enum PauseReason {
    Manual,
    Step,
    Breakpoint { method: u32, pc: u32 },
    MethodEntry { method: u32 },
    Throw { class_desc: String, message: String },
    UnsupportedApi { detail: String },
    TaskExit { task: u32 },
    Uncaught { class_desc: String, message: String, task: u32 },
    VmFault { detail: String },
    Budget,
    Idle,
}

#[derive(Debug, Default)]
pub struct DebugState {
    /// (global method idx, insn idx) exact instruction breakpoints
    pub breakpoints: HashSet<(u32, u32)>,
    /// global method idx -> break on first instruction
    pub method_breakpoints: HashSet<u32>,
    /// (class name, method name) breakpoints matched before resolution
    pub named_breakpoints: HashSet<(String, String)>,
    pub step_remaining: u64,
    pub paused: bool,
    pub last_reason: Option<PauseReason>,
    pub instruction_count: u64,
    /// method entry/exit trace depth limit (0 = off)
    pub trace_depth: u32,
}

impl DebugState {
    pub fn check_break(&self, method: u32, pc: u32) -> bool {
        self.breakpoints.contains(&(method, pc)) || self.method_breakpoints.contains(&method)
    }
}
