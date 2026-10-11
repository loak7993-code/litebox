//! Cooperative task model: frames, stacks, states, and the round-robin
//! scheduler queue. Tasks are green threads; the debugger pauses and steps
//! them individually.

use crate::decode_types::MethodCode;
use crate::value::Value;
use std::collections::VecDeque;
use std::sync::Arc;

#[derive(Clone)]
pub struct Frame {
    /// global method index in the registry
    pub method: u32,
    pub code: Arc<MethodCode>,
    /// decoded instruction index
    pub pc: usize,
    pub regs: Box<[Value]>,
    pub class_id: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskState {
    Runnable,
    Sleeping { until_us: u64 },
    /// blocked on a monitor held by another task
    WaitingLock { obj: u32 },
    /// Object.wait(): waiting for notify; optional timeout in virtual us
    WaitNotify { obj: u32, until_us: Option<u64> },
    Joining { target: u32 },
    Done,
}

impl TaskState {
    pub fn is_runnable(&self) -> bool {
        matches!(self, TaskState::Runnable)
    }
}

#[derive(Clone)]
pub struct Task {
    pub id: u32,
    pub name: String,
    pub state: TaskState,
    pub stack: Vec<Frame>,
    /// result of the last invoke, consumed by move-result
    pub pending_result: Option<Value>,
    /// reference of the exception being handled (move-exception)
    pub thrown: Option<Value>,
    /// uncaught exception that terminated the task
    pub death_exn: Option<crate::errors::JavaExn>,
    /// sequence number for deterministic tie-breaks
    pub seq: u64,
}

impl Task {
    pub fn current_frame(&self) -> Option<&Frame> {
        self.stack.last()
    }
    pub fn depth(&self) -> usize {
        self.stack.len()
    }
}

#[derive(Default)]
pub struct TaskSet {
    pub tasks: Vec<Option<Task>>,
    pub queue: VecDeque<u32>,
    pub next_id: u32,
    pub seq_counter: u64,
    /// scheduler hint for which task most recently ran
    pub current_hint: u32,
    /// per-object monitors: obj idx -> (owner task, count, waiters)
    pub monitors: std::collections::HashMap<u32, MonitorState>,
}

#[derive(Debug, Clone, Default)]
pub struct MonitorState {
    pub owner: Option<u32>,
    pub count: u32,
    pub waiters: Vec<u32>,
}

impl TaskSet {
    pub fn spawn(&mut self, name: &str) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        self.seq_counter += 1;
        self.tasks.push(Some(Task {
            id,
            name: name.to_string(),
            state: TaskState::Runnable,
            stack: Vec::new(),
            pending_result: None,
            thrown: None,
            death_exn: None,
            seq: self.seq_counter,
        }));
        self.queue.push_back(id);
        id
    }

    pub fn task(&self, id: u32) -> Option<&Task> {
        self.tasks.get(id as usize).and_then(|t| t.as_ref())
    }

    pub fn task_mut(&mut self, id: u32) -> Option<&mut Task> {
        self.tasks.get_mut(id as usize).and_then(|t| t.as_mut())
    }

    pub fn live_count(&self) -> usize {
        self.tasks.iter().filter(|t| t.is_some()).count()
    }

    pub fn runnable_ids(&self) -> Vec<u32> {
        self.tasks
            .iter()
            .filter_map(|t| t.as_ref())
            .filter(|t| t.state.is_runnable())
            .map(|t| t.id)
            .collect()
    }

    /// Pick the next runnable task, round-robin by queue order.
    pub fn pick_next(&mut self) -> Option<u32> {
        for _ in 0..self.queue.len() {
            let id = self.queue.pop_front().unwrap();
            let runnable = self.task(id).map(|t| t.state.is_runnable()).unwrap_or(false);
            self.queue.push_back(id);
            if runnable {
                return Some(id);
            }
        }
        None
    }

    pub fn next_wake_us(&self) -> Option<u64> {
        let mut min: Option<u64> = None;
        for t in self.tasks.iter().flatten() {
            match t.state {
                TaskState::Sleeping { until_us } | TaskState::WaitNotify { until_us: Some(until_us), .. } => {
                    min = Some(min.map_or(until_us, |m| m.min(until_us)));
                }
                _ => {}
            }
        }
        min
    }

    pub fn reap_done(&mut self) {
        // keep task records for post-mortem debugging; reap only on explicit
        // vm_reset. Done tasks leave the queue naturally.
    }
}
