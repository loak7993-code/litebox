//! The execution engine: machine state, the dispatch loop, invoke and
//! exception machinery, class initialization, and the scheduler.
//!
//! Borrow strategy: the current frame's register box is swapped out with
//! `mem::take` for the duration of one instruction dispatch, so the heap,
//! registry, and task set can mutate freely inside any opcode arm. Frame
//! stack mutations (invoke push, return pop, throw unwind) also happen inside
//! dispatch since the registers are a local value at that point.
//!
//! GC deferral: allocations inside dispatch cannot safely collect (the local
//! register box is not a root yet), so allocation helpers only set
//! `gc_pending`; the run loops drain it between instructions.

use crate::classes::{ClassKind, Registry, Target, ACC_ENUM};
use crate::debug::{DebugState, PauseReason};
use crate::decode_types::{
    Cond, CmpOp, ConvOp, FieldRef, FillPayload, Ins, MathOp, MethodCode, MethodRef, SwitchTable,
};
use crate::errors::{JavaExn, ThrowSpec, VmFault};
use crate::heap::{ArrayKind, Heap, ObjectData};
use crate::tasks::{Frame, TaskSet, TaskState};
use crate::classes::zero_for;
use crate::value::{narrow, ObjRef, PrimKind, Value, NULL};
use litebox_core::config::PauseOn;
use litebox_core::{EventBus, EventLevel, Rng, SandboxFs, Screen, VmClock};
use crate::env::Env;
use std::collections::HashMap;
use std::sync::Arc;

pub struct Machine {
    pub heap: Heap,
    pub registry: Registry,
    pub clock: VmClock,
    pub rng: Rng,
    pub bus: EventBus,
    pub fs: SandboxFs,
    pub tasks: TaskSet,
    pub screen: Screen,
    pub string_intern: HashMap<String, ObjRef>,
    pub debug: DebugState,
    pub heap_budget: u64,
    pub slice: u64,
    pub pause_on_exception: bool,
    pub pause_on_unsupported: bool,
    pub max_instructions: u64,
    pub wall_clock_start_ms: u64,
    pub stats: VmStats,
    pub gc_pending: bool,
    pub nested_depth: u32,
    /// deferred rewind used by the debugger: when set, the next step re-throws
    /// this exception before anything else executes
    pub pending_throw: Option<JavaExn>,
}

#[derive(Debug, Default, Clone)]
pub struct VmStats {
    pub instructions: u64,
    pub invokes: u64,
    pub native_calls: u64,
    pub exceptions_thrown: u64,
    pub allocations: u64,
    pub class_inits: u64,
    pub task_switches: u64,
    pub unsupported_calls: u64,
}

#[derive(Debug)]
pub enum RunOutcome {
    Paused(PauseReason),
    Settled { alive: usize, blocked: usize },
    Faulted(String, u32),
}

enum Flow {
    Next(usize),
    Jump(usize),
    /// current frame returned; caller pc already advanced by dispatch
    Return,
    /// a dex frame was pushed; the caller pc stays at the invoke site (a
    /// normal return advances it, an unwind searches handlers there)
    Invoked,
}

enum DispatchErr {
    Java(JavaExn),
    Spec(ThrowSpec),
    Fault(String),
    Yield,
}

enum StepErr {
    Java(JavaExn),
    Fault(String),
    Yield,
}

pub enum Control {
    Done(Value),
    Java(JavaExn),
    Fault(String),
    /// debugger pause inside a debuggable call; frames remain for inspection
    Paused(PauseReason),
}

/// Outcome of an agent-driven (debuggable) call.
pub enum CallOutcome {
    Returned(Value),
    Paused(PauseReason),
}

impl Machine {
    pub fn new(cfg: &litebox_core::Config) -> Machine {
        Machine {
            heap: Heap::new(),
            registry: Registry::new(),
            clock: VmClock::new(),
            rng: Rng::new(cfg.seed),
            bus: EventBus::new(cfg.event_capacity),
            fs: SandboxFs::new(),
            tasks: TaskSet::default(),
            screen: Screen::new(cfg.surface_width, cfg.surface_height),
            string_intern: HashMap::new(),
            debug: DebugState::default(),
            heap_budget: cfg.heap_budget_bytes,
            slice: cfg.slice_instructions,
            pause_on_exception: cfg.pause_on != PauseOn::Off,
            pause_on_unsupported: cfg.pause_on == PauseOn::ExceptionAndUnsupported,
            max_instructions: cfg.max_instructions,
            wall_clock_start_ms: cfg.wall_clock_start_ms,
            stats: VmStats::default(),
            gc_pending: false,
            nested_depth: 0,
            pending_throw: None,
        }
    }

    /// Ring capacity used when rebuilding the bus on restore.
    pub(crate) fn bus_capacity_hint(&self) -> usize {
        self.bus.next_seq().saturating_sub(self.bus.dropped()) as usize + 1024
    }

    pub fn log(&mut self, level: EventLevel, tag: &str, msg: &str) {
        let ts = self.clock.now_us();
        self.bus.push(ts, level, tag, msg, None);
    }

    pub fn log_data(&mut self, level: EventLevel, tag: &str, msg: &str, data: litebox_json::Json) {
        let ts = self.clock.now_us();
        self.bus.push(ts, level, tag, msg, Some(data));
    }

    // ---------- object/string helpers for the framework layer ----------

    pub fn make_str(&mut self, s: &str) -> Value {
        if let Some(&r) = self.string_intern.get(s) {
            if self.heap.is_live(r) {
                return Value::R(r);
            }
            self.string_intern.remove(s);
        }
        let r = self.heap.alloc(ObjectData::Str(s.into()), (s.len() + 24) as u32);
        self.string_intern.insert(s.to_string(), r);
        self.stats.allocations += 1;
        self.request_gc();
        Value::R(r)
    }

    pub fn str_of(&self, r: ObjRef) -> Option<String> {
        match self.heap.get(r) {
            Some(ObjectData::Str(s)) => Some(s.to_string()),
            _ => None,
        }
    }

    /// Replace a heap object's content in place (used by String ctors).
    pub fn heap_set_str(&mut self, r: ObjRef, s: &str) -> bool {
        self.heap.replace(r, ObjectData::Str(s.into()))
    }

    pub fn str_value(&self, v: Value) -> Option<String> {
        if v.is_null() {
            return None;
        }
        self.str_of(v.as_ref())
    }

    pub fn object_class_of(&self, v: Value) -> Option<u32> {
        if v.is_null() {
            return None;
        }
        match self.heap.get(v.as_ref()) {
            Some(ObjectData::Instance { class, .. }) => Some(*class),
            Some(ObjectData::Str(_)) => self.registry.class_by_name("Ljava/lang/String;"),
            Some(ObjectData::Array { .. }) | Some(ObjectData::Class(_)) => {
                self.registry.class_by_name("Ljava/lang/Object;")
            }
            None => None,
        }
    }

    pub fn alloc_instance(&mut self, class_id: u32) -> Result<ObjRef, VmFault> {
        let slots = self.registry.instance_slots(class_id);
        let fields = vec![Value::U; slots.max(1)].into_boxed_slice();
        let r = self
            .heap
            .alloc(ObjectData::Instance { class: class_id, fields }, 64 + slots as u32 * 16);
        self.stats.allocations += 1;
        self.request_gc();
        Ok(r)
    }

    pub fn alloc_instance_named(&mut self, desc: &str) -> Result<ObjRef, ThrowSpec> {
        let tid = self.registry.intern_type(desc);
        let cid = self
            .registry
            .ensure_class(tid)
            .map_err(|e| ThrowSpec::new("Ljava/lang/NoClassDefFoundError;", e.to_string()))?;
        self.init_class(cid)
            .map_err(|e| match e {
                VmFault::Java(j) => ThrowSpec::new(&j.class_desc, self.exn_message(&j)),
                VmFault::Vm(m) => ThrowSpec::new("Ljava/lang/VmError;", m),
            })?;
        self.alloc_instance(cid)
            .map_err(|e| ThrowSpec::new("Ljava/lang/VmError;", e.to_string()))
    }

    pub fn get_instance_field(&self, obj: ObjRef, name: &str) -> Option<Value> {
        let cid = match self.heap.get(obj) {
            Some(ObjectData::Instance { class, .. }) => *class,
            _ => return None,
        };
        let mut cur = Some(cid);
        while let Some(c) = cur {
            let cls = &self.registry.classes[c as usize];
            if let Some(f) =
                cls.instance_fields.iter().find(|f| self.registry.name_str(f.name) == name)
            {
                let fields = match self.heap.get(obj) {
                    Some(ObjectData::Instance { fields, .. }) => fields,
                    _ => return None,
                };
                return fields.get(f.slot as usize).copied();
            }
            cur = cls.super_id;
        }
        None
    }

    pub fn set_instance_field(&mut self, obj: ObjRef, name: &str, v: Value) -> bool {
        let cid = match self.heap.get(obj) {
            Some(ObjectData::Instance { class, .. }) => *class,
            _ => return false,
        };
        let mut slot = None;
        let mut cur = Some(cid);
        while let Some(c) = cur {
            let cls = &self.registry.classes[c as usize];
            if let Some(f) =
                cls.instance_fields.iter().find(|f| self.registry.name_str(f.name) == name)
            {
                slot = Some(f.slot);
                break;
            }
            cur = cls.super_id;
        }
        if let Some(s) = slot {
            if let Some(ObjectData::Instance { fields, .. }) = self.heap.get_mut(obj) {
                fields[s as usize] = v;
                return true;
            }
        }
        false
    }

    pub fn exn_message(&self, exn: &JavaExn) -> String {
        self.get_instance_field(exn.obj, "detailMessage")
            .and_then(|v| self.str_value(v))
            .unwrap_or_default()
    }

    /// Build a Java exception object, initialize it, capture the stack trace.
    pub fn raise(&mut self, spec: &ThrowSpec) -> Result<JavaExn, VmFault> {
        self.stats.exceptions_thrown += 1;
        let tid = self.registry.intern_type(&spec.class);
        let cid = self.registry.ensure_class(tid)?;
        self.init_class(cid)?;
        let obj = self.alloc_instance(cid)?;
        if !spec.msg.is_empty() {
            let m = self.make_str(&spec.msg);
            self.set_instance_field(obj, "detailMessage", m);
        }
        for (k, v) in &spec.data {
            let dv = self.make_str(v);
            self.set_instance_field(obj, k, dv);
        }
        let mut frames = Vec::new();
        let cur = self.tasks.current_hint;
        if let Some(task) = self.tasks.task(cur) {
            for f in task.stack.iter().rev() {
                let me = self.registry.method_at(f.method);
                let cls = self.registry.class_name(f.class_id).to_string();
                let mn =
                    me.map(|m| self.registry.name_str(m.name).to_string()).unwrap_or_default();
                frames.push(crate::heap::ExFrame {
                    class_name: cls,
                    method_name: mn,
                    line: f.pc as u32,
                });
            }
        }
        self.heap.exception_frames.insert(obj.idx, frames.clone());
        Ok(JavaExn { obj, class_desc: spec.class.clone(), frames })
    }

    pub fn fault_to_spec(&self, e: &VmFault) -> ThrowSpec {
        match e {
            VmFault::Java(j) => ThrowSpec::new(&j.class_desc, self.exn_message(j)),
            VmFault::Vm(m) => ThrowSpec::new("Ljava/lang/VmError;", m.clone()),
        }
    }

    /// Convert a ThrowSpec to a JavaExn now (allocating the object).
    pub fn exn_of(&mut self, spec: &ThrowSpec) -> Result<JavaExn, VmFault> {
        self.raise(spec)
    }

    fn request_gc(&mut self) {
        if self.heap.live_bytes > self.heap_budget {
            self.gc_pending = true;
        }
    }

    /// Collect unreachable objects. Called between instructions only.
    pub fn do_gc(&mut self) {
        self.gc_pending = false;
        let mut roots: Vec<ObjRef> = Vec::new();
        for t in self.tasks.tasks.iter().flatten() {
            if let Some(Value::R(r)) = &t.pending_result {
                roots.push(*r);
            }
            if let Some(Value::R(r)) = &t.thrown {
                roots.push(*r);
            }
            for f in &t.stack {
                for v in f.regs.iter() {
                    if let Value::R(r) = v {
                        roots.push(*r);
                    }
                }
            }
        }
        for c in &self.registry.classes {
            for s in &c.statics {
                if let Value::R(r) = s.value {
                    roots.push(r);
                }
            }
        }
        for r in self.string_intern.values() {
            roots.push(*r);
        }
        for obj in self.heap.exception_frames.keys() {
            roots.push(ObjRef { idx: *obj, gen: 0 });
        }
        let root_list = roots;
        self.heap.collect(move |visit| {
            for &r in &root_list {
                visit(r);
            }
        });
        self.log(
            EventLevel::Debug,
            "dalvik",
            &format!(
                "gc: live={} heap_bytes={} gcs={}",
                self.heap.len(),
                self.heap.live_bytes,
                self.heap.gc_count
            ),
        );
    }

    // ---------- arrays ----------

    pub fn alloc_array(&mut self, kind: ArrayKind, len: usize) -> ObjRef {
        let fill = zero_for(match kind {
            ArrayKind::Longs => PrimKind::J,
            ArrayKind::Floats => PrimKind::F,
            ArrayKind::Doubles => PrimKind::D,
            ArrayKind::Refs => PrimKind::Ref,
            _ => PrimKind::I,
        });
        let r = self.heap.alloc(
            ObjectData::Array { kind, data: vec![fill; len].into_boxed_slice() },
            (32 + len * 16) as u32,
        );
        self.stats.allocations += 1;
        self.request_gc();
        r
    }

    pub fn make_byte_array(&mut self, bytes: &[u8]) -> Value {
        let r = self.alloc_array(ArrayKind::Ints, bytes.len());
        if let Some(ObjectData::Array { data, .. }) = self.heap.get_mut(r) {
            *data =
                bytes.iter().map(|&b| Value::I(b as i32)).collect::<Vec<_>>().into_boxed_slice();
        }
        Value::R(r)
    }

    pub fn byte_array_of(&self, v: Value) -> Option<Vec<u8>> {
        if v.is_null() {
            return None;
        }
        match self.heap.get(v.as_ref()) {
            Some(ObjectData::Array { kind: ArrayKind::Ints, data }) => Some(
                data.iter()
                    .map(|x| match x {
                        Value::I(i) => *i as u8,
                        _ => 0,
                    })
                    .collect(),
            ),
            _ => None,
        }
    }

    pub fn make_str_array(&mut self, items: &[String]) -> Value {
        let vals: Vec<Value> = items.iter().map(|s| self.make_str(s)).collect();
        let r = self.alloc_array(ArrayKind::Refs, vals.len());
        if let Some(ObjectData::Array { data, .. }) = self.heap.get_mut(r) {
            *data = vals.into_boxed_slice();
        }
        Value::R(r)
    }

    // ---------- class initialization ----------

    pub fn init_class(&mut self, class_id: u32) -> Result<(), VmFault> {
        if self.registry.classes[class_id as usize].initialized
            || self.registry.classes[class_id as usize].init_in_progress
        {
            return Ok(());
        }
        self.registry.classes[class_id as usize].init_in_progress = true;
        self.nested_depth += 1;
        let res = self.init_class_inner(class_id);
        self.nested_depth -= 1;
        self.registry.classes[class_id as usize].init_in_progress = false;
        if res.is_ok() {
            self.registry.classes[class_id as usize].initialized = true;
        }
        res
    }

    fn init_class_inner(&mut self, class_id: u32) -> Result<(), VmFault> {
        let super_id = self.registry.classes[class_id as usize].super_id;
        if let Some(sc) = super_id {
            self.init_class(sc)?;
        }
        let kind = self.registry.classes[class_id as usize].kind.clone();
        if let ClassKind::Dex { dex, def } = kind {
            let dex_ref = self.registry.dexes[dex as usize].clone();
            let cd = &dex_ref.class_defs[def as usize];
            let values: Vec<(usize, Value)> = cd
                .static_values
                .iter()
                .enumerate()
                .map(|(i, ev)| (i, self.enc_to_value(ev, &dex_ref)))
                .collect();
            for (i, v) in values {
                if let Some(s) = self.registry.classes[class_id as usize].statics.get_mut(i) {
                    s.value = v;
                }
            }
            let nid = self.registry.intern_name("<clinit>");
            let clinit = self.registry.classes[class_id as usize].dmap.get(&(nid, 0)).copied();
            if let Some(gidx) = clinit {
                self.stats.class_inits += 1;
                self.run_method_to_completion(class_id, gidx, Vec::new())?;
            }
            let is_enum = self.registry.classes[class_id as usize].access & ACC_ENUM != 0;
            if is_enum {
                let cls_name = self.registry.class_name(class_id).to_string();
                let mut consts: Vec<(usize, String, u32)> = Vec::new();
                for (i, s) in self.registry.classes[class_id as usize].statics.iter().enumerate() {
                    if self.registry.type_str(s.desc) == cls_name {
                        consts.push((i, self.registry.name_str(s.name).to_string(), i as u32));
                    }
                }
                for (idx, name, ordinal) in consts {
                    let obj = self.alloc_instance(class_id)?;
                    let nref = self.make_str(&name);
                    self.set_instance_field(obj, "name", nref);
                    self.set_instance_field(obj, "ordinal", Value::I(ordinal as i32));
                    self.registry.classes[class_id as usize].statics[idx].value = Value::R(obj);
                }
            }
        }
        self.registry.classes[class_id as usize].initialized = true;
        Ok(())
    }

    fn enc_to_value(&mut self, ev: &litebox_dex::EncodedValue, _dex: &litebox_dex::Dex) -> Value {
        use litebox_dex::EncodedValue as EV;
        match ev {
            EV::Byte(v) => Value::I(*v as i32),
            EV::Short(v) => Value::I(*v as i32),
            EV::Char(v) => Value::I(*v as i32),
            EV::Int(v) => Value::I(*v),
            EV::Long(v) => Value::L(*v),
            EV::Float(v) => Value::F(*v),
            EV::Double(v) => Value::D(*v),
            EV::Bool(v) => Value::I(*v as i32),
            EV::Null => NULL,
            EV::Enum(_) | EV::MethodType(_) | EV::Field(_) | EV::Method(_) => NULL,
            EV::Array(items) => {
                let vals: Vec<Value> =
                    items.iter().map(|e| self.enc_to_value(e, _dex)).collect();
                let r = self.alloc_array(ArrayKind::Refs, vals.len());
                if let Some(ObjectData::Array { data, .. }) = self.heap.get_mut(r) {
                    *data = vals.into_boxed_slice();
                }
                Value::R(r)
            }
        }
    }

    // ---------- task/frame plumbing ----------

    pub fn spawn_task_method(
        &mut self,
        name: &str,
        gidx: u32,
        args: Vec<Value>,
    ) -> Result<u32, VmFault> {
        let task = self.tasks.spawn(name);
        let this = NULL;
        self.invoke_global(task, gidx, this, args)?;
        Ok(task)
    }

    /// Ensure a runnable main task exists for nested/agent-driven calls.
    /// A task that died (uncaught exception) is replaced with a fresh one so
    /// the agent can keep driving the VM.
    pub fn ensure_main_task(&mut self) -> u32 {
        let dead = self
            .tasks
            .task(self.tasks.current_hint)
            .map(|t| t.state == TaskState::Done)
            .unwrap_or(true);
        if dead {
            let id = self.tasks.spawn("main");
            self.tasks.current_hint = id;
        }
        self.tasks.current_hint
    }

    /// Push a frame (or run a native) for a global method onto `task`.
    pub fn invoke_global(
        &mut self,
        task: u32,
        gidx: u32,
        this: Value,
        args: Vec<Value>,
    ) -> Result<(), VmFault> {
        let entry = self
            .registry
            .method_at(gidx)
            .cloned()
            .ok_or_else(|| VmFault::Vm("bad method idx".into()))?;
        match &entry.target {
            Target::Dex { .. } => {
                let code = entry
                    .code
                    .clone()
                    .ok_or_else(|| VmFault::Vm("abstract method invoked".into()))?;
                let nregs = code.registers_size as usize;
                let mut regs = vec![Value::U; nregs.max(1)].into_boxed_slice();
                let mut slot: i64 =
                    nregs as i64 - (entry.params.iter().sum::<u8>() as i64)
                        - if entry.is_static { 0 } else { 1 };
                if slot < 0 {
                    return Err(VmFault::Vm("ins_size smaller than args".into()));
                }
                if !entry.is_static {
                    regs[slot as usize] = this;
                    slot += 1;
                }
                for (i, &w) in entry.params.iter().enumerate() {
                    regs[slot as usize] = args.get(i).copied().unwrap_or(Value::U);
                    slot += w as i64;
                }
                self.tasks.task_mut(task).unwrap().stack.push(Frame {
                    method: gidx,
                    code,
                    pc: 0,
                    regs,
                    class_id: entry.class_id,
                });
                Ok(())
            }
            Target::Native { class, name, shorty } => {
                let desc = self.registry.class_name(*class).to_string();
                let nname = self.registry.name_str(*name).to_string();
                let nshorty = self.registry.shorty_str(*shorty).to_string();
                // natives get `this` as the dedicated second parameter; the
                // args slice holds only the declared parameters
                let full_args: Vec<Value> = args;
                let f = self
                    .registry
                    .natives
                    .lookup(&desc, &nname, &nshorty)
                    .map(|(f, _)| f.clone())
                    .ok_or_else(|| {
                        VmFault::Vm(format!(
                            "native not registered: {desc}.{nname}{nshorty}"
                        ))
                    })?;
                self.stats.native_calls += 1;
                let mut env = Env { m: self };
                let result = f(&mut env, if entry.is_static { NULL } else { this }, &full_args);
                let t = env.m.tasks.task_mut(task).unwrap();
                match result {
                    Ok(v) => t.pending_result = Some(v),
                    Err(spec) => {
                        let exn = env.m.raise(&spec)?;
                        return Err(VmFault::Java(exn));
                    }
                }
                Ok(())
            }
        }
    }

    /// Agent-driven call: breakpoints and steps pause mid-call with the
    /// frames intact; the agent can inspect, then resume.
    pub fn call_method_debuggable(
        &mut self,
        this: Value,
        class_desc: &str,
        method: &str,
        shorty: &str,
        args: Vec<Value>,
    ) -> Result<CallOutcome, JavaExn> {
        self.nested_depth += 1;
        let out = self.call_method_debuggable_inner(this, class_desc, method, shorty, args);
        self.nested_depth -= 1;
        out
    }

    fn call_method_debuggable_inner(
        &mut self,
        this: Value,
        class_desc: &str,
        method: &str,
        shorty: &str,
        args: Vec<Value>,
    ) -> Result<CallOutcome, JavaExn> {
        self.ensure_main_task();
        let tid = self.registry.intern_type(class_desc);
        let named_cid = self.registry.ensure_class(tid).map_err(|e| {
            self.blocking_exn(ThrowSpec::new("Ljava/lang/NoClassDefFoundError;", e.to_string()))
        })?;
        self.init_class(named_cid)
            .map_err(|e| self.blocking_exn(self.fault_to_spec(&e)))?;
        let cid = if this.is_null() {
            named_cid
        } else {
            let oc = self.object_class_of(this).unwrap_or(named_cid);
            let t = self.registry.type_of_class(oc);
            self.registry.ensure_class(t).unwrap_or(oc)
        };
        let nid = self.registry.intern_name(method);
        let sid = self.registry.intern_shorty(&sig_to_shorty(shorty));
        let slots = parse_sig(shorty).iter().sum::<u8>() as u32;
        let cls = &self.registry.classes[cid as usize];
        let gidx = cls
            .dmap
            .get(&(nid, slots))
            .copied()
            .or_else(|| cls.vmap.get(&(nid, sid)).copied())
            .ok_or_else(|| {
                self.blocking_exn(ThrowSpec::new(
                    "Ljava/lang/NoSuchMethodError;",
                    format!("{class_desc}.{method}{shorty}"),
                ))
            })?;
        let task = self.tasks.current_hint;
        let depth = self.tasks.task(task).map(|t| t.stack.len()).unwrap_or(0);
        self.invoke_global(task, gidx, this, args)
            .map_err(|e| match e {
                VmFault::Java(exn) => exn,
                VmFault::Vm(m) => self.blocking_exn(ThrowSpec::new("Ljava/lang/VmError;", m)),
            })?;
        self.debug.paused = false;
        match self.run_to_depth(task, depth) {
            Control::Done(v) => Ok(CallOutcome::Returned(v)),
            Control::Java(exn) => Err(exn),
            Control::Fault(m) => Err(self.blocking_exn(ThrowSpec::new("Ljava/lang/VmError;", m))),
            Control::Paused(reason) => Ok(CallOutcome::Paused(reason)),
        }
    }

    /// Invoke and run to completion (natives calling back, agent `call`).
    pub fn call_method_nested(
        &mut self,
        this: Value,
        class_desc: &str,
        method: &str,
        shorty: &str,
        args: Vec<Value>,
    ) -> Result<Value, JavaExn> {
        self.nested_depth += 1;
        let result = self.call_method_nested_inner(this, class_desc, method, shorty, args);
        self.nested_depth -= 1;
        result
    }

    fn call_method_nested_inner(
        &mut self,
        this: Value,
        class_desc: &str,
        method: &str,
        shorty: &str,
        args: Vec<Value>,
    ) -> Result<Value, JavaExn> {
        self.ensure_main_task();
        let tid = self.registry.intern_type(class_desc);
        let named_cid = self.registry.ensure_class(tid).map_err(|e| {
            self.blocking_exn(ThrowSpec::new("Ljava/lang/NoClassDefFoundError;", e.to_string()))
        })?;
        self.init_class(named_cid)
            .map_err(|e| self.blocking_exn(self.fault_to_spec(&e)))?;
        // instance calls dispatch on the receiver's RUNTIME class (virtual
        // semantics); the named class is the fallback for statics/null
        let cid = if this.is_null() {
            named_cid
        } else {
            let oc = self.object_class_of(this).unwrap_or(named_cid);
            // force shim finalization for runtime-resolved receivers
            let t = self.registry.type_of_class(oc);
            self.registry.ensure_class(t).unwrap_or(oc)
        };
        let nid = self.registry.intern_name(method);
        let sid = self.registry.intern_shorty(&sig_to_shorty(shorty));
        let slots = parse_sig(shorty).iter().sum::<u8>() as u32;
        let (dmap_hit, vmap_hit, _dbg_dmap, _dbg_vmap) = {
            let cls = &self.registry.classes[cid as usize];
            (
                cls.dmap.get(&(nid, slots)).copied(),
                cls.vmap.get(&(nid, sid)).copied(),
                cls.dmap.iter().take(12).map(|(k, _)| *k).collect::<Vec<_>>(),
                cls.vmap.iter().take(12).map(|(k, _)| *k).collect::<Vec<_>>(),
            )
        };
        let gidx = dmap_hit.or(vmap_hit);
        let gidx = match gidx {
            Some(g) => g,
            None => {
                
                return Err(self.blocking_exn(ThrowSpec::new(
                    "Ljava/lang/NoSuchMethodError;",
                    format!("{class_desc}.{method}{shorty}"),
                )));
            }
        };
        let task = self.tasks.current_hint;
        let depth = self.tasks.task(task).map(|t| t.stack.len()).unwrap_or(0);
        self.invoke_global(task, gidx, this, args)
            .map_err(|e| match e {
                VmFault::Java(exn) => exn,
                VmFault::Vm(m) => {
                    self.blocking_exn(ThrowSpec::new("Ljava/lang/VmError;", m))
                }
            })?;
        match self.run_to_depth(task, depth) {
            Control::Done(v) => Ok(v),
            Control::Java(exn) => Err(exn),
            Control::Fault(m) => {
                Err(self.blocking_exn(ThrowSpec::new("Ljava/lang/VmError;", m)))
            }
            Control::Paused(reason) => Err(self.blocking_exn(ThrowSpec::new(
                "Ljava/lang/VmError;",
                format!("debugger pause inside nested call: {reason:?}"),
            ))),
        }
    }

    /// Nested contexts cannot pause; failures become exceptions.
    fn blocking_exn(&mut self, spec: ThrowSpec) -> JavaExn {
        match self.raise(&spec) {
            Ok(e) => e,
            Err(_) => JavaExn {
                obj: crate::value::ObjRef { idx: crate::heap::NULL_IDX, gen: 0 },
                class_desc: spec.class,
                frames: Vec::new(),
            },
        }
    }

    fn run_method_to_completion(
        &mut self,
        _class_id: u32,
        gidx: u32,
        args: Vec<Value>,
    ) -> Result<Value, VmFault> {
        self.nested_depth += 1;
        let out = self.run_method_to_completion_inner(gidx, args);
        self.nested_depth -= 1;
        out
    }

    fn run_method_to_completion_inner(
        &mut self,
        gidx: u32,
        args: Vec<Value>,
    ) -> Result<Value, VmFault> {
        let task = self.tasks.current_hint;
        let depth = self.tasks.task(task).map(|t| t.stack.len()).unwrap_or(0);
        self.invoke_global(task, gidx, NULL, args)?;
        match self.run_to_depth(task, depth) {
            Control::Done(v) => Ok(v),
            Control::Java(exn) => Err(VmFault::Java(exn)),
            Control::Fault(m) => Err(VmFault::Vm(m)),
            Control::Paused(reason) => Err(VmFault::Vm(format!("debugger pause inside class init: {reason:?}"))),
        }
    }

    fn run_to_depth(&mut self, task: u32, depth: usize) -> Control {
        for _ in 0..20_000_000u64 {
            if self.gc_pending {
                self.do_gc();
            }
            let (stack_len, frame_method, frame_pc) = {
                let Some(t) = self.tasks.task(task) else {
                    return Control::Fault("task gone".into());
                };
                match t.stack.last() {
                    Some(f) => (t.stack.len(), f.method, f.pc as u32),
                    None => (0, 0, 0),
                }
            };
            if stack_len <= depth {
                let v = self.tasks.task_mut(task).unwrap().pending_result.take().unwrap_or(NULL);
                return Control::Done(v);
            }
            if self.tasks.task(task).map(|t| t.state) != Some(TaskState::Runnable) {
                return Control::Done(NULL);
            }
            // debugger checks apply inside debuggable calls too
            if self.debug.check_break(frame_method, frame_pc) {
                self.debug.paused = true;
                let reason = PauseReason::Breakpoint { method: frame_method, pc: frame_pc };
                self.debug.last_reason = Some(reason.clone());
                return Control::Paused(reason);
            }
            if self.debug.step_remaining > 0 {
                self.debug.step_remaining -= 1;
                if self.debug.step_remaining == 0 {
                    self.debug.paused = true;
                    self.debug.last_reason = Some(PauseReason::Step);
                    return Control::Paused(PauseReason::Step);
                }
            }
            match self.step_one(task) {
                Ok(()) => {}
                Err(StepErr::Java(exn)) => {
                    if let Err(uncaught) = self.handle_throw(task, exn) {
                        return Control::Java(uncaught);
                    }
                }
                Err(StepErr::Fault(m)) => return Control::Fault(m),
                Err(StepErr::Yield) => {
                    return Control::Done(NULL);
                }
            }
        }
        Control::Fault("nested call budget exhausted".into())
    }

    // ---------- scheduler ----------

    /// Wake sleepers whose virtual deadline passed; advance the clock when
    /// nothing can run. Returns false when the set is empty or deadlocked.
    pub fn scheduler_tick(&mut self) -> bool {
        let now = self.clock.now_us();
        let task_ids: Vec<u32> = self.tasks.tasks.iter().flatten().map(|t| t.id).collect();
        for id in task_ids {
            let Some(t) = self.tasks.task_mut(id) else { continue };
            match t.state {
                TaskState::Sleeping { until_us } if until_us <= now => {
                    t.state = TaskState::Runnable;
                }
                TaskState::WaitNotify { obj, until_us: Some(u) } if u <= now => {
                    t.state = TaskState::Runnable;
                    if let Some(m) = self.tasks.monitors.get_mut(&obj) {
                        if m.owner.is_none() {
                            m.owner = Some(id);
                            m.count = 1;
                        }
                    }
                }
                _ => {}
            }
        }
        if self.tasks.pick_next().is_some() {
            return true;
        }
        if let Some(next) = self.tasks.next_wake_us() {
            self.clock.advance_to(next as u128);
            return true;
        }
        false
    }

    /// Run until a pause, all tasks settle, or a fault.
    pub fn run_until_pause(&mut self) -> RunOutcome {
        if let Some(exn) = self.pending_throw.take() {
            let task = self.tasks.current_hint;
            if let Err(uncaught) = self.handle_throw(task, exn) {
                self.death_by_uncaught(task, &uncaught);
                return RunOutcome::Paused(PauseReason::Uncaught {
                    class_desc: uncaught.class_desc.clone(),
                    message: self.exn_message(&uncaught),
                    task,
                });
            }
        }
        loop {
            if !self.scheduler_tick() {
                let alive = self.tasks.live_count();
                let done =
                    self.tasks.tasks.iter().flatten().filter(|t| t.state == TaskState::Done).count();
                return RunOutcome::Settled { alive, blocked: alive.saturating_sub(done) };
            }
            let Some(task) = self.tasks.pick_next() else { continue };
            self.tasks.current_hint = task;
            self.stats.task_switches += 1;
            let budget = if self.debug.step_remaining > 0 {
                self.debug.step_remaining
            } else {
                self.slice
            };
            match self.run_slice(task, budget) {
                RunOutcome::Paused(reason) => {
                    if matches!(reason, PauseReason::Step) {
                        self.debug.step_remaining = 0;
                    }
                    return RunOutcome::Paused(reason);
                }
                RunOutcome::Faulted(msg, t) => return RunOutcome::Faulted(msg, t),
                RunOutcome::Settled { .. } => {}
            }
        }
    }

    fn death_by_uncaught(&mut self, task: u32, exn: &JavaExn) {
        if let Some(t) = self.tasks.task_mut(task) {
            t.state = TaskState::Done;
            t.death_exn = Some(exn.clone());
        }
        let msg = self.exn_message(exn);
        self.log_data(
            EventLevel::Error,
            "dalvik",
            &format!("uncaught exception in task {task}: {} {}", exn.class_desc, msg),
            litebox_json::Json::obj(vec![
                ("task", litebox_json::Json::u32n(task)),
                ("class", litebox_json::Json::str(exn.class_desc.clone())),
                ("message", litebox_json::Json::str(msg.clone())),
                (
                    "frames",
                    litebox_json::Json::Arr(
                        exn.frames
                            .iter()
                            .map(|f| {
                                litebox_json::Json::obj(vec![
                                    ("class", litebox_json::Json::str(f.class_name.clone())),
                                    ("method", litebox_json::Json::str(f.method_name.clone())),
                                    ("pc", litebox_json::Json::u32n(f.line)),
                                ])
                            })
                            .collect(),
                    ),
                ),
            ]),
        );
    }

    /// Execute up to `budget` instructions on one task, pausing per debugger
    /// policy (breakpoints, step budget, exception pause).
    pub fn run_slice(&mut self, task: u32, mut budget: u64) -> RunOutcome {
        self.tasks.current_hint = task;
        while budget > 0 {
            budget -= 1;
            if self.max_instructions > 0
                && self.debug.instruction_count >= self.max_instructions
            {
                self.debug.paused = true;
                self.debug.last_reason = Some(PauseReason::Budget);
                return RunOutcome::Paused(PauseReason::Budget);
            }
            if self.gc_pending {
                self.do_gc();
            }
            // deferred unwind from a pause-on-throw
            if let Some(exn) = self.pending_throw.take() {
                match self.handle_throw(task, exn) {
                    Ok(()) => {}
                    Err(uncaught) => {
                        self.death_by_uncaught(task, &uncaught);
                        self.debug.paused = true;
                        self.debug.last_reason = Some(PauseReason::Uncaught {
                            class_desc: uncaught.class_desc.clone(),
                            message: self.exn_message(&uncaught),
                            task,
                        });
                        return RunOutcome::Paused(self.debug.last_reason.clone().unwrap());
                    }
                }
            }
            let Some(t) = self.tasks.task(task) else {
                return RunOutcome::Settled { alive: 0, blocked: 0 };
            };
            match t.state {
                TaskState::Done => {
                    return RunOutcome::Settled { alive: 0, blocked: 0 };
                }
                TaskState::Runnable => {}
                _ => return RunOutcome::Settled { alive: 1, blocked: 1 },
            }
            if t.stack.is_empty() {
                let t = self.tasks.task_mut(task).unwrap();
                t.state = TaskState::Done;
                return RunOutcome::Settled { alive: 0, blocked: 0 };
            }
            // debugger checks before executing
            let (frame_method, frame_pc) = {
                let f = t.stack.last().unwrap();
                (f.method, f.pc as u32)
            };
            if self.debug.paused {
                return RunOutcome::Paused(
                    self.debug.last_reason.clone().unwrap_or(PauseReason::Manual),
                );
            }
            if self.debug.check_break(frame_method, frame_pc) {
                self.debug.paused = true;
                self.debug.last_reason =
                    Some(PauseReason::Breakpoint { method: frame_method, pc: frame_pc });
                return RunOutcome::Paused(self.debug.last_reason.clone().unwrap());
            }
            if self.debug.step_remaining > 0 {
                self.debug.step_remaining -= 1;
                if self.debug.step_remaining == 0 {
                    self.debug.paused = true;
                    self.debug.last_reason = Some(PauseReason::Step);
                    return RunOutcome::Paused(PauseReason::Step);
                }
            }
            match self.step_one(task) {
                Ok(()) => {}
                Err(StepErr::Yield) => {
                    return RunOutcome::Settled { alive: 1, blocked: 1 };
                }
                Err(StepErr::Fault(m)) => {
                    self.log(EventLevel::Error, "dalvik", &format!("vm fault task {task}: {m}"));
                    let t = self.tasks.task_mut(task).unwrap();
                    t.state = TaskState::Done;
                    return RunOutcome::Faulted(m, task);
                }
                Err(StepErr::Java(exn)) => {
                    let pause_here = self.pause_on_exception && self.nested_depth == 0;
                    if pause_here {
                        self.debug.paused = true;
                        let msg = self.exn_message(&exn);
                        self.debug.last_reason = Some(PauseReason::Throw {
                            class_desc: exn.class_desc.clone(),
                            message: msg,
                        });
                        self.pending_throw = Some(exn);
                        return RunOutcome::Paused(self.debug.last_reason.clone().unwrap());
                    }
                    if let Err(uncaught) = self.handle_throw(task, exn) {
                        self.death_by_uncaught(task, &uncaught);
                        let msg = self.exn_message(&uncaught);
                        self.debug.paused = true;
                        self.debug.last_reason = Some(PauseReason::Uncaught {
                            class_desc: uncaught.class_desc.clone(),
                            message: msg,
                            task,
                        });
                        return RunOutcome::Paused(self.debug.last_reason.clone().unwrap());
                    }
                }
            }
        }
        RunOutcome::Settled { alive: 1, blocked: 0 }
    }

    fn step_one(&mut self, task: u32) -> Result<(), StepErr> {
        let Some(t) = self.tasks.task(task) else {
            return Err(StepErr::Fault("task gone".into()));
        };
        if t.state != TaskState::Runnable {
            return Ok(());
        }
        let Some(frame) = t.stack.last() else {
            return Err(StepErr::Fault("empty stack".into()));
        };
        let method = frame.method;
        let class_id = frame.class_id;
        let code = frame.code.clone();
        let pc = frame.pc;
        let depth = t.stack.len(); // the executing frame's position (1-based)
        let pending = self.tasks.task_mut(task).unwrap().pending_result.take();
        self.debug.instruction_count += 1;
        self.stats.instructions += 1;

        let regs = std::mem::take(
            &mut self.tasks.task_mut(task).unwrap().stack.last_mut().unwrap().regs,
        );
        let (regs, result) = self.dispatch(task, class_id, &code, regs, pc, pending);
        // Put registers back into the frame they came from. Position-based:
        // the owning frame sat at index depth-1. After an invoke push the
        // callee sits ABOVE it (ambiguous by identity alone when recursing).
        let code_clone = code.clone();
        let mut regs = regs;
        if let Some(t) = self.tasks.task_mut(task) {
            if t.stack.len() >= depth {
                let f = &mut t.stack[depth - 1];
                if Arc::ptr_eq(&f.code, &code_clone) && f.method == method {
                    f.regs = regs;
                    regs = Box::new([]);
                }
            }
        }
        drop(regs);
        match result {
            Ok(flow) => match flow {
                Flow::Next(n) | Flow::Jump(n) => {
                    let t = self.tasks.task_mut(task).unwrap();
                    if t.stack.len() >= depth {
                        let f = &mut t.stack[depth - 1];
                        if Arc::ptr_eq(&f.code, &code_clone) && f.method == method {
                            f.pc = n;
                        }
                    }
                    Ok(())
                }
                // frame pushed: the callee executes next; the caller pc is
                // already correct (invoke site); registers were restored above
                Flow::Invoked => Ok(()),
                Flow::Return => Ok(()),
            },
            Err(DispatchErr::Java(exn)) => Err(StepErr::Java(exn)),
            Err(DispatchErr::Spec(spec)) => {
                let exn = self.exn_of(&spec).map_err(|e| match e {
                    VmFault::Java(j) => StepErr::Java(j),
                    VmFault::Vm(m) => StepErr::Fault(m),
                })?;
                Err(StepErr::Java(exn))
            }
            Err(DispatchErr::Fault(m)) => Err(StepErr::Fault(m)),
            Err(DispatchErr::Yield) => Err(StepErr::Yield),
        }
    }

    /// Search exception handlers from the top frame down. Returns Err(uncaught)
    /// if no handler anywhere.
    fn handle_throw(&mut self, task: u32, exn: JavaExn) -> Result<(), JavaExn> {
        let exn_cid = {
            let tid = self.registry.intern_type(&exn.class_desc);
            match self.registry.class_by_type(tid) {
                Some(c) => c,
                None => return Err(exn),
            }
        };
        loop {
            let Some(t) = self.tasks.task_mut(task) else { return Err(exn.clone()) };
            let Some(frame) = t.stack.last() else {
                t.state = TaskState::Done;
                t.death_exn = Some(exn.clone());
                return Err(exn);
            };
            let pc = frame.pc;
            let code = frame.code.clone();
            let mut caught: Option<usize> = None;
            for (start, count, hidx) in &code.tries {
                let s = *start as usize;
                let c = *count as usize;
                if s <= pc && pc < s + c {
                    let h = &code.handlers[*hidx];
                    for (catch_tid, handler_pc) in &h.typed {
                        if let Some(ccid) = self.registry.class_by_type(*catch_tid) {
                            if self.registry.subtype(exn_cid, ccid) {
                                caught = Some(*handler_pc as usize);
                                break;
                            }
                        }
                    }
                    if caught.is_none() {
                        if let Some(all_pc) = h.catch_all {
                            caught = Some(all_pc as usize);
                        }
                    }
                    if caught.is_some() {
                        break;
                    }
                }
            }
            if let Some(handler_pc) = caught {
                t.thrown = Some(Value::R(exn.obj));
                let f = t.stack.last_mut().unwrap();
                f.pc = handler_pc;
                return Ok(());
            }
            // pop and continue in the caller
            t.stack.pop();
            if t.stack.is_empty() {
                t.state = TaskState::Done;
                t.death_exn = Some(exn.clone());
                return Err(exn);
            }
        }
    }

    // ---------- the dispatch loop ----------

    #[allow(clippy::too_many_arguments)]
    fn dispatch(
        &mut self,
        task: u32,
        class_id: u32,
        code: &Arc<MethodCode>,
        mut regs: Box<[Value]>,
        pc: usize,
        pending: Option<Value>,
    ) -> (Box<[Value]>, Result<Flow, DispatchErr>) {
        let next = pc + 1;
        macro_rules! ret {
            ($v:expr) => {{
                return (regs, $v);
            }};
        }
        let ins: &Ins = match code.insns.get(pc) {
            Some(i) => i,
            None => {
                ret!(Err(DispatchErr::Fault(format!("pc {} out of range", pc))));
            }
        };
        // a helper so every arm can `regs` mutably then continue
        let result: Result<Flow, DispatchErr> = match ins {
            Ins::Nop => Ok(Flow::Next(next)),
            Ins::Move { dst, src } => {
                regs[*dst as usize] = regs[*src as usize];
                Ok(Flow::Next(next))
            }
            Ins::MoveWide { dst, src } => {
                regs[*dst as usize] = regs[*src as usize];
                Ok(Flow::Next(next))
            }
            Ins::MoveObj { dst, src } => {
                regs[*dst as usize] = regs[*src as usize];
                Ok(Flow::Next(next))
            }
            Ins::MoveResult { dst, wide } => {
                let v = pending.unwrap_or(NULL);
                regs[*dst as usize] = v;
                let _ = wide;
                Ok(Flow::Next(next))
            }
            Ins::MoveException { dst } => {
                let v = self.tasks.task_mut(task).and_then(|t| t.thrown.take()).unwrap_or(NULL);
                regs[*dst as usize] = v;
                Ok(Flow::Next(next))
            }
            Ins::Return { reg, .. } => {
                let v = regs[*reg as usize];
                self.tasks.task_mut(task).unwrap().pending_result = Some(v);
                let t = self.tasks.task_mut(task).unwrap();
                t.stack.pop();
                if let Some(caller) = t.stack.last_mut() {
                    caller.pc += 1;
                }
                Ok(Flow::Return)
            }
            Ins::ReturnVoid => {
                self.tasks.task_mut(task).unwrap().pending_result = Some(NULL);
                let t = self.tasks.task_mut(task).unwrap();
                t.stack.pop();
                if let Some(caller) = t.stack.last_mut() {
                    caller.pc += 1;
                }
                Ok(Flow::Return)
            }
            Ins::ConstI32 { dst, v } => {
                regs[*dst as usize] = Value::I(*v);
                Ok(Flow::Next(next))
            }
            Ins::ConstI64 { dst, v } => {
                regs[*dst as usize] = Value::L(*v);
                Ok(Flow::Next(next))
            }
            Ins::ConstF32 { dst, v } => {
                regs[*dst as usize] = Value::F(*v);
                Ok(Flow::Next(next))
            }
            Ins::ConstF64 { dst, v } => {
                regs[*dst as usize] = Value::D(*v);
                Ok(Flow::Next(next))
            }
            Ins::ConstString { dst, s } => {
                let text = self.registry.string_text(*s).to_string();
                regs[*dst as usize] = self.make_str(&text);
                Ok(Flow::Next(next))
            }
            Ins::ConstClass { dst, class } => match self.const_class_obj(*class) {
                Ok(v) => {
                    regs[*dst as usize] = v;
                    Ok(Flow::Next(next))
                }
                Err(e) => Err(e),
            },
            Ins::CheckCast { reg, class } => {
                let v = regs[*reg as usize];
                if v.is_null() {
                    Ok(Flow::Next(next))
                } else {
                    match self.object_class_of(v) {
                        Some(oc) => {
                            let target_cid = match self.registry.class_by_type(*class) {
                                Some(c) => c,
                                None => match self.registry.ensure_class(*class) {
                                    Ok(c) => c,
                                    Err(e) => return (regs, Err(DispatchErr::Fault(e.to_string()))),
                                },
                            };
                            if self.registry.subtype(oc, target_cid) {
                                Ok(Flow::Next(next))
                            } else {
                                Err(DispatchErr::Spec(ThrowSpec::new(
                                    "Ljava/lang/ClassCastException;",
                                    format!(
                                        "{} cannot be cast to {}",
                                        self.registry.class_name(oc),
                                        self.registry.type_str(*class)
                                    ),
                                )))
                            }
                        }
                        None => Err(DispatchErr::Spec(ThrowSpec::npe("check-cast on null class"))),
                    }
                }
            }
            Ins::InstanceOf { dst, obj, class } => {
                let v = regs[*obj as usize];
                let res = if v.is_null() {
                    0
                } else {
                    match self.object_class_of(v) {
                        Some(oc) => {
                            let target_cid = match self.registry.class_by_type(*class) {
                                Some(c) => c,
                                None => {
                                    match self.registry.ensure_class(*class) {
                                        Ok(c) => c,
                                        Err(e) => {
                                            return (regs, Err(DispatchErr::Fault(e.to_string())))
                                        }
                                    }
                                }
                            };
                            if self.registry.subtype(oc, target_cid) {
                                1
                            } else {
                                0
                            }
                        }
                        None => 0,
                    }
                };
                regs[*dst as usize] = Value::I(res);
                Ok(Flow::Next(next))
            }
            Ins::ArrayLen { dst, arr } => {
                let v = regs[*arr as usize];
                if v.is_null() {
                    Err(DispatchErr::Spec(ThrowSpec::npe("array-length on null")))
                } else {
                    match self.heap.get(v.as_ref()) {
                        Some(ObjectData::Array { data, .. }) => {
                            regs[*dst as usize] = Value::I(data.len() as i32);
                            Ok(Flow::Next(next))
                        }
                        _ => Err(DispatchErr::Spec(ThrowSpec::npe("array-length on non-array"))),
                    }
                }
            }
            Ins::NewInstance { dst, class } => {
                match self.init_class_for(*class) {
                    Ok(cid) => {
                        if self.registry.is_abstract_class(cid) || self.registry.is_interface(cid) {
                            Err(DispatchErr::Spec(ThrowSpec::new(
                                "Ljava/lang/InstantiationError;",
                                self.registry.class_name(cid).to_string(),
                            )))
                        } else {
                            match self.alloc_instance(cid) {
                                Ok(r) => {
                                    regs[*dst as usize] = Value::R(r);
                                    Ok(Flow::Next(next))
                                }
                                Err(e) => Err(DispatchErr::Fault(e.to_string())),
                            }
                        }
                    }
                    Err(e) => Err(e),
                }
            }
            Ins::NewArray { dst, len, elem_desc } => {
                let l = regs[*len as usize].as_i32();
                if l < 0 {
                    Err(DispatchErr::Spec(ThrowSpec::new(
                        "Ljava/lang/NegativeArraySizeException;",
                        l.to_string(),
                    )))
                } else {
                    let kind = array_kind_of_desc(self, *elem_desc);
                    let r = self.alloc_array(kind, l as usize);
                    regs[*dst as usize] = Value::R(r);
                    Ok(Flow::Next(next))
                }
            }
            Ins::FilledNewArray { regs: rlist, count, elem_desc } => {
                let kind = array_kind_of_desc(self, *elem_desc);
                let n = *count as usize;
                let mut vals = Vec::with_capacity(n);
                for i in 0..n {
                    let reg = rlist.get(i).copied().unwrap_or(0);
                    vals.push(regs.get(reg as usize).copied().unwrap_or(NULL));
                }
                let arr = self.alloc_array(kind, vals.len());
                if let Some(ObjectData::Array { data, .. }) = self.heap.get_mut(arr) {
                    *data = vals.into_boxed_slice();
                }
                self.tasks.task_mut(task).unwrap().pending_result = Some(Value::R(arr));
                Ok(Flow::Next(next))
            }
            Ins::FillArrayData { arr, data } => {
                let v = regs[*arr as usize];
                if v.is_null() {
                    Err(DispatchErr::Spec(ThrowSpec::npe("fill-array-data on null")))
                } else {
                    match data.as_ref() {
                        FillPayload::Data { width, data } => match self.heap.get_mut(v.as_ref()) {
                            Some(ObjectData::Array { kind, data: dst }) => {
                                if fill_into(dst, *kind, *width, data) {
                                    Ok(Flow::Next(next))
                                } else {
                                    Err(DispatchErr::Fault("fill payload width mismatch".into()))
                                }
                            }
                            _ => Err(DispatchErr::Spec(ThrowSpec::npe("fill on non-array"))),
                        },
                    }
                }
            }
            Ins::Switch { reg, kind, table } => {
                let v = regs[*reg as usize].as_i32();
                let target = match (kind, table.as_ref()) {
                    (crate::decode_types::SwitchKind::Packed, SwitchTable::Packed { first, targets }) => {
                        let idx = (v - *first) as usize;
                        if v >= *first && idx < targets.len() {
                            Some(targets[idx])
                        } else {
                            None
                        }
                    }
                    (crate::decode_types::SwitchKind::Sparse, SwitchTable::Sparse { keys, targets }) => {
                        keys.binary_search(&v).ok().map(|i| targets[i])
                    }
                    _ => None,
                };
                match target {
                    Some(t) => Ok(Flow::Jump(t as usize)),
                    None => Ok(Flow::Next(next)),
                }
            }
            Ins::Throw { reg } => {
                let v = regs[*reg as usize];
                if v.is_null() {
                    Err(DispatchErr::Spec(ThrowSpec::npe("throw null")))
                } else {
                    let obj = v.as_ref();
                    let desc = match self.heap.get(obj) {
                        Some(ObjectData::Instance { class, .. }) => {
                            self.registry.class_name(*class).to_string()
                        }
                        _ => "Ljava/lang/Throwable;".to_string(),
                    };
                    Err(DispatchErr::Java(JavaExn { obj, class_desc: desc, frames: Vec::new() }))
                }
            }
            Ins::Goto { target } => Ok(Flow::Jump(*target as usize)),
            Ins::Branch { cond, a, b, target } => {
                let va = regs[*a as usize].as_i32();
                let vb = regs[*b as usize].as_i32();
                let taken = match cond {
                    Cond::Eq => va == vb,
                    Cond::Ne => va != vb,
                    Cond::Lt => va < vb,
                    Cond::Ge => va >= vb,
                    Cond::Gt => va > vb,
                    Cond::Le => va <= vb,
                };
                if taken {
                    Ok(Flow::Jump(*target as usize))
                } else {
                    Ok(Flow::Next(next))
                }
            }
            Ins::BranchZ { cond, a, target } => {
                let taken = match cond {
                    Cond::Eq => !regs[*a as usize].truthy(),
                    Cond::Ne => regs[*a as usize].truthy(),
                    Cond::Lt => regs[*a as usize].as_i32() < 0,
                    Cond::Ge => regs[*a as usize].as_i32() >= 0,
                    Cond::Gt => regs[*a as usize].as_i32() > 0,
                    Cond::Le => regs[*a as usize].as_i32() <= 0,
                };
                if taken {
                    Ok(Flow::Jump(*target as usize))
                } else {
                    Ok(Flow::Next(next))
                }
            }
            Ins::Cmp { op, dst, a, b } => {
                let va = regs[*a as usize];
                let vb = regs[*b as usize];
                let r = match op {
                    CmpOp::FloatL => cmp_f(as_float(va), as_float(vb), -1),
                    CmpOp::FloatG => cmp_f(as_float(va), as_float(vb), 1),
                    CmpOp::DoubleL => cmp_d(as_double(va), as_double(vb), -1),
                    CmpOp::DoubleG => cmp_d(as_double(va), as_double(vb), 1),
                    CmpOp::Long => {
                        let x = va.as_i64();
                        let y = vb.as_i64();
                        if x < y {
                            -1
                        } else if x > y {
                            1
                        } else {
                            0
                        }
                    }
                };
                regs[*dst as usize] = Value::I(r);
                Ok(Flow::Next(next))
            }
            Ins::Math { op, wide, float, dst, a, b } => {
                let (dstv, err) = if *float {
                    if *wide {
                        math_d(*op, as_double(regs[*a as usize]), as_double(regs[*b as usize]))
                    } else {
                        math_f(*op, as_float(regs[*a as usize]), as_float(regs[*b as usize]))
                    }
                } else if *wide {
                    math_l(*op, regs[*a as usize].as_i64(), regs[*b as usize].as_i64())
                } else {
                    math_i(*op, regs[*a as usize].as_i32(), regs[*b as usize].as_i32())
                };
                match err {
                    Some(spec) => Err(DispatchErr::Spec(spec)),
                    None => {
                        regs[*dst as usize] = dstv;
                        Ok(Flow::Next(next))
                    }
                }
            }
            Ins::MathLit { op, dst, a, lit } => {
                let (v, err) = math_i(*op, regs[*a as usize].as_i32(), *lit);
                match err {
                    Some(spec) => Err(DispatchErr::Spec(spec)),
                    None => {
                        regs[*dst as usize] = v;
                        Ok(Flow::Next(next))
                    }
                }
            }
            Ins::Math2Addr { op, wide, float, a, b } => {
                let (dstv, err) = if *float {
                    if *wide {
                        math_d(*op, as_double(regs[*a as usize]), as_double(regs[*b as usize]))
                    } else {
                        math_f(*op, as_float(regs[*a as usize]), as_float(regs[*b as usize]))
                    }
                } else if *wide {
                    math_l(*op, regs[*a as usize].as_i64(), regs[*b as usize].as_i64())
                } else {
                    math_i(*op, regs[*a as usize].as_i32(), regs[*b as usize].as_i32())
                };
                match err {
                    Some(spec) => Err(DispatchErr::Spec(spec)),
                    None => {
                        regs[*a as usize] = dstv;
                        Ok(Flow::Next(next))
                    }
                }
            }
            Ins::Conv { op, dst, a } => {
                let va = regs[*a as usize];
                regs[*dst as usize] = convert(*op, va);
                Ok(Flow::Next(next))
            }
            Ins::AGet { elem_desc, dst, arr, idx } => {
                let av = regs[*arr as usize];
                let iv = regs[*idx as usize].as_i32();
                if av.is_null() {
                    Err(DispatchErr::Spec(ThrowSpec::npe("aget on null array")))
                } else {
                    match self.heap.get(av.as_ref()) {
                        Some(ObjectData::Array { data, .. }) => {
                            if iv < 0 || iv as usize >= data.len() {
                                Err(DispatchErr::Spec(ThrowSpec::ioob(format!(
                                    "index {iv}, length {}",
                                    data.len()
                                ))))
                            } else {
                                let mut v = data[iv as usize];
                                if let Some(b) = first_byte_desc(self, *elem_desc) {
                                    v = narrow(b, v);
                                }
                                regs[*dst as usize] = v;
                                Ok(Flow::Next(next))
                            }
                        }
                        _ => Err(DispatchErr::Spec(ThrowSpec::npe("aget on non-array"))),
                    }
                }
            }
            Ins::APut { elem_desc, src, arr, idx } => {
                let av = regs[*arr as usize];
                let iv = regs[*idx as usize].as_i32();
                if av.is_null() {
                    Err(DispatchErr::Spec(ThrowSpec::npe("aput on null array")))
                } else {
                    let val = regs[*src as usize];
                    let narrowed = first_byte_desc(self, *elem_desc).map(|b| narrow(b, val)).unwrap_or(val);
                    match self.heap.get_mut(av.as_ref()) {
                        Some(ObjectData::Array { data, .. }) => {
                            if iv < 0 || iv as usize >= data.len() {
                                let len = data.len();
                                let spec = ThrowSpec::ioob(format!("index {iv}, length {len}"));
                                return (regs, Err(DispatchErr::Spec(spec)));
                            } else {
                                data[iv as usize] = narrowed;
                                Ok(Flow::Next(next))
                            }
                        }
                        _ => Err(DispatchErr::Spec(ThrowSpec::npe("aput on non-array"))),
                    }
                }
            }
            Ins::IGet { field, dst, obj } => match self.field_ref_target(field) {
                Ok(ft) => {
                    let ov = regs[*obj as usize];
                    if ov.is_null() {
                        Err(DispatchErr::Spec(ThrowSpec::npe(format!(
                            "iget on null: {}",
                            field_desc_str(self, field)
                        ))))
                    } else {
                        match self.heap.get(ov.as_ref()) {
                            Some(ObjectData::Instance { fields, .. }) => {
                                match ft {
                                    crate::classes::FieldTarget::Instance { slot, .. } => {
                                        regs[*dst as usize] =
                                            fields.get(slot as usize).copied().unwrap_or(NULL);
                                        Ok(Flow::Next(next))
                                    }
                                    crate::classes::FieldTarget::Static { .. } => {
                                        // static accessed via iget: legal in dex
                                        let v = self.registry.get_static(ft);
                                        regs[*dst as usize] = v;
                                        Ok(Flow::Next(next))
                                    }
                                }
                            }
                            _ => Err(DispatchErr::Spec(ThrowSpec::npe("iget on non-object"))),
                        }
                    }
                }
                Err(e) => Err(DispatchErr::Fault(e.to_string())),
            },
            Ins::IPut { field, src, obj } => match self.field_ref_target(field) {
                Ok(ft) => {
                    let ov = regs[*obj as usize];
                    if ov.is_null() {
                        Err(DispatchErr::Spec(ThrowSpec::npe(format!(
                            "iput on null: {}",
                            field_desc_str(self, field)
                        ))))
                    } else {
                        let v = regs[*src as usize];
                        match ft {
                            crate::classes::FieldTarget::Instance { slot, .. } => {
                                match self.heap.get_mut(ov.as_ref()) {
                                    Some(ObjectData::Instance { fields, .. }) => {
                                        if (slot as usize) < fields.len() {
                                            fields[slot as usize] = v;
                                            Ok(Flow::Next(next))
                                        } else {
                                            Err(DispatchErr::Fault("field slot oob".into()))
                                        }
                                    }
                                    _ => Err(DispatchErr::Spec(ThrowSpec::npe("iput on non-object"))),
                                }
                            }
                            crate::classes::FieldTarget::Static { .. } => {
                                self.registry.set_static(ft, v);
                                Ok(Flow::Next(next))
                            }
                        }
                    }
                }
                Err(e) => Err(DispatchErr::Fault(e.to_string())),
            },
            Ins::SGet { field, dst } => match self.field_ref_target(field) {
                Ok(ft) => match ft {
                    crate::classes::FieldTarget::Static { .. } => {
                        let v = self.registry.get_static(ft);
                        regs[*dst as usize] = v;
                        Ok(Flow::Next(next))
                    }
                    crate::classes::FieldTarget::Instance { .. } => {
                        // static instruction hitting an instance field: error
                        Err(DispatchErr::Spec(ThrowSpec::new(
                            "Ljava/lang/IncompatibleClassChangeError;",
                            field_desc_str(self, field),
                        )))
                    }
                },
                Err(e) => Err(DispatchErr::Fault(e.to_string())),
            },
            Ins::SPut { field, src } => match self.field_ref_target(field) {
                Ok(ft) => match ft {
                    crate::classes::FieldTarget::Static { .. } => {
                        let v = regs[*src as usize];
                        self.registry.set_static(ft, v);
                        Ok(Flow::Next(next))
                    }
                    crate::classes::FieldTarget::Instance { .. } => Err(DispatchErr::Spec(
                        ThrowSpec::new("Ljava/lang/IncompatibleClassChangeError;", field_desc_str(self, field)),
                    )),
                },
                Err(e) => Err(DispatchErr::Fault(e.to_string())),
            },
            Ins::Invoke { method: mref, regs: rlist, count } => {
                let argregs: Vec<u8> =
                    rlist.iter().take(*count as usize).copied().collect();
                self.do_invoke(task, mref, &argregs, &regs, class_id, next, code, pc)
            }
            Ins::InvokeRange { method: mref, start, count } => {
                let argregs: Vec<u8> = (0..*count).map(|i| start.wrapping_add(i)).collect();
                self.do_invoke(task, mref, &argregs, &regs, class_id, next, code, pc)
            }
            Ins::MonitorEnter { obj } => {
                let v = regs[*obj as usize];
                if v.is_null() {
                    Err(DispatchErr::Spec(ThrowSpec::npe("monitor-enter on null")))
                } else {
                    let obj_idx = v.as_ref().idx;
                    let m = self.tasks.monitors.entry(obj_idx).or_default();
                    match m.owner {
                        None => {
                            m.owner = Some(task);
                            m.count = 1;
                            Ok(Flow::Next(next))
                        }
                        Some(owner) if owner == task => {
                            m.count += 1;
                            Ok(Flow::Next(next))
                        }
                        Some(_) => {
                            // cooperative block
                            let t = self.tasks.task_mut(task).unwrap();
                            t.state = TaskState::WaitingLock { obj: obj_idx };
                            Err(DispatchErr::Yield)
                        }
                    }
                }
            }
            Ins::MonitorExit { obj } => {
                let v = regs[*obj as usize];
                if v.is_null() {
                    Err(DispatchErr::Spec(ThrowSpec::npe("monitor-exit on null")))
                } else {
                    let obj_idx = v.as_ref().idx;
                    let mut released = false;
                    if let Some(m) = self.tasks.monitors.get_mut(&obj_idx) {
                        if m.owner == Some(task) {
                            m.count -= 1;
                            if m.count == 0 {
                                m.owner = None;
                                released = true;
                            }
                        }
                    }
                    if released {
                        // wake a waiter if any
                        let waiter = self
                            .tasks
                            .monitors
                            .get(&obj_idx)
                            .and_then(|m| m.waiters.first().copied());
                        if let Some(w) = waiter {
                            if let Some(m) = self.tasks.monitors.get_mut(&obj_idx) {
                                m.waiters.remove(0);
                            }
                            let t = self.tasks.task_mut(w).unwrap();
                            t.state = TaskState::Runnable;
                            let mm = self.tasks.monitors.get_mut(&obj_idx).unwrap();
                            mm.owner = Some(w);
                            mm.count = 1;
                        }
                    }
                    Ok(Flow::Next(next))
                }
            }
        };
        (regs, result)
    }

    #[allow(clippy::too_many_arguments)]
    fn do_invoke(
        &mut self,
        task: u32,
        mref: &MethodRef,
        argregs: &[u8],
        regs: &[Value],
        caller_class: u32,
        next: usize,
        code: &Arc<MethodCode>,
        pc: usize,
    ) -> Result<Flow, DispatchErr> {
        self.stats.invokes += 1;
        match mref {
            MethodRef::Direct { dex, method_idx, resolved, .. } => {
                let gidx = if *resolved != u32::MAX {
                    Ok(*resolved)
                } else {
                    self.registry
                        .resolve_direct(*dex, *method_idx)
                        .map_err(|e| DispatchErr::Fault(e.to_string()))
                };
                let gidx = gidx?;
                // inline cache: rewrite the instruction's resolved target
                if *resolved == u32::MAX {
                    patch_direct_resolved(code, pc, gidx);
                }
                self.invoke_resolved(task, gidx, argregs, regs, next)
            }
            MethodRef::Virtual { name, shorty, is_interface } => {
                let this = argregs.first().and_then(|&r| regs.get(r as usize)).copied().unwrap_or(NULL);
                if this.is_null() {
                    let cname = self.registry.name_str(*name).to_string();
                    return Err(DispatchErr::Spec(ThrowSpec::npe(format!(
                        "invoke on null receiver: {}",
                        cname
                    ))));
                }
                let oc = match self.object_class_of(this) {
                    Some(c) => c,
                    None => {
                        return Err(DispatchErr::Spec(ThrowSpec::npe("invoke on unresolvable object")))
                    }
                };
                let mut cur = Some(oc);
                let mut found = None;
                while let Some(c) = cur {
                    if let Some(&g) = self.registry.classes[c as usize].vmap.get(&(*name, *shorty)) {
                        found = Some(g);
                        break;
                    }
                    cur = self.registry.classes[c as usize].super_id;
                }
                let gidx = match found {
                    Some(g) => g,
                    None => {
                        // interface dispatch: look in the receiver's imap
                        let mut ifound = None;
                        let mut icur = Some(oc);
                        while let Some(c) = icur {
                            for m in self.registry.classes[c as usize].imap.values() {
                                if let Some(&g) = m.get(&(*name, *shorty)) {
                                    ifound = Some(g);
                                    break;
                                }
                            }
                            if ifound.is_some() {
                                break;
                            }
                            icur = self.registry.classes[c as usize].super_id;
                        }
                        match ifound {
                            Some(g) => g,
                            None => {
                                let nm = self.registry.name_str(*name).to_string();
                                let sh = self.registry.shorty_str(*shorty).to_string();
                                let detail = format!(
                                    "{}.{}({})",
                                    self.registry.class_name(oc),
                                    nm,
                                    sh
                                );
                                if self.chain_has_shim(oc) {
                                    return Err(DispatchErr::Spec(ThrowSpec::unsupported(detail)));
                                }
                                return Err(DispatchErr::Spec(ThrowSpec::new(
                                    "Ljava/lang/NoSuchMethodError;",
                                    format!("{detail}{}", if *is_interface { " (interface)" } else { "" }),
                                )));
                            }
                        }
                    }
                };
                self.invoke_resolved(task, gidx, argregs, regs, next)
            }
            MethodRef::Super { name, shorty } => {
                // dispatch in the caller's superclass
                let super_cid = self
                    .registry
                    .classes
                    .get(caller_class as usize)
                    .and_then(|c| c.super_id)
                    .ok_or_else(|| {
                        DispatchErr::Spec(ThrowSpec::new(
                            "Ljava/lang/NoSuchMethodError;",
                            "invoke-super without superclass",
                        ))
                    })?;
                let mut cur = Some(super_cid);
                let mut found = None;
                while let Some(c) = cur {
                    if let Some(&g) = self.registry.classes[c as usize].vmap.get(&(*name, *shorty)) {
                        found = Some(g);
                        break;
                    }
                    cur = self.registry.classes[c as usize].super_id;
                }
                let gidx = found.ok_or_else(|| {
                    let detail = format!(
                        "{}.{}({}) via super",
                        self.registry.class_name(super_cid),
                        self.registry.name_str(*name),
                        self.registry.shorty_str(*shorty)
                    );
                    if self.chain_has_shim(super_cid) {
                        DispatchErr::Spec(ThrowSpec::unsupported(detail))
                    } else {
                        DispatchErr::Spec(ThrowSpec::new("Ljava/lang/NoSuchMethodError;", detail))
                    }
                })?;
                self.invoke_resolved(task, gidx, argregs, regs, next)
            }
            MethodRef::Unsupported { op } => Err(DispatchErr::Spec(ThrowSpec::unsupported(format!(
                "invoke opcode {op:#04x} (polymorphic/custom calls) is not implemented"
            )))),
        }
    }

    fn invoke_resolved(
        &mut self,
        task: u32,
        gidx: u32,
        argregs: &[u8],
        regs: &[Value],
        next: usize,
    ) -> Result<Flow, DispatchErr> {
        let entry = self
            .registry
            .method_at(gidx)
            .cloned()
            .ok_or_else(|| DispatchErr::Fault("bad method idx".into()))?;
        // class init for static invocations and constructors
        if entry.is_static || entry.name == self.registry.name_id("<init>").unwrap_or(u32::MAX) {
            let cid = entry.class_id;
            self.init_class(cid).map_err(|e| match e {
                VmFault::Java(j) => DispatchErr::Java(j),
                VmFault::Vm(m) => DispatchErr::Fault(m),
            })?;
        }
        if entry.is_abstract {
            return Err(DispatchErr::Spec(ThrowSpec::new(
                "Ljava/lang/AbstractMethodError;",
                format!(
                    "{}.{}",
                    self.registry.class_name(entry.class_id),
                    self.registry.name_str(entry.name)
                ),
            )));
        }
        let is_static = entry.is_static;
        // walk the register list per parameter slot width
        let val_at = |c: usize| -> Value {
            argregs.get(c).and_then(|&r| regs.get(r as usize)).copied().unwrap_or(NULL)
        };
        let mut cursor = 0usize;
        let this = if is_static { NULL } else { val_at(cursor) };
        if !is_static {
            cursor += 1;
        }
        let mut args: Vec<Value> = Vec::with_capacity(entry.params.len());
        for &w in &entry.params {
            args.push(val_at(cursor));
            cursor += w as usize;
        }
        match &entry.target {
            Target::Dex { .. } => {
                let code = entry
                    .code
                    .clone()
                    .ok_or_else(|| DispatchErr::Fault("no code".into()))?;
                let nregs = code.registers_size as usize;
                let mut newregs = vec![Value::U; nregs.max(1)].into_boxed_slice();
                let mut slot: i64 = nregs as i64
                    - (entry.params.iter().sum::<u8>() as i64)
                    - if is_static { 0 } else { 1 };
                if slot < 0 {
                    return Err(DispatchErr::Fault("ins_size smaller than args".into()));
                }
                if !is_static {
                    newregs[slot as usize] = this;
                    slot += 1;
                }
                for (i, &w) in entry.params.iter().enumerate() {
                    newregs[slot as usize] = args.get(i).copied().unwrap_or(Value::U);
                    slot += w as i64;
                }
                self.tasks.task_mut(task).unwrap().stack.push(Frame {
                    method: gidx,
                    code,
                    pc: 0,
                    regs: newregs,
                    class_id: entry.class_id,
                });
                // caller pc stays at the invoke site for unwinding; a normal
                // return advances it
                Ok(Flow::Invoked)
            }
            Target::Native { class, name, shorty } => {
                let desc = self.registry.class_name(*class).to_string();
                let nname = self.registry.name_str(*name).to_string();
                let nshorty = self.registry.shorty_str(*shorty).to_string();
                let Some((f, _)) = self.registry.natives.lookup(&desc, &nname, &nshorty) else {
                    self.stats.unsupported_calls += 1;
                    let detail = format!("{}({}) via {}", desc, nname, nshorty);
                    self.log(EventLevel::Warn, "framework", &format!("api not implemented: {detail}"));
                    return Err(DispatchErr::Spec(ThrowSpec::unsupported(detail)));
                };
                let f = f.clone();
                self.stats.native_calls += 1;
                let full_args: Vec<Value> = args;
                let mut env = Env { m: self };
                let result = f(&mut env, if is_static { NULL } else { this }, &full_args);
                let t = env.m.tasks.task_mut(task).unwrap();
                match result {
                    Ok(v) => {
                        t.pending_result = Some(v);
                        Ok(Flow::Next(next))
                    }
                    Err(spec) => {
                        if spec.class == "Llitebox/UnsupportedApiException;" {
                            env.m.stats.unsupported_calls += 1;
                        }
                        match env.m.raise(&spec) {
                            Ok(exn) => Err(DispatchErr::Java(exn)),
                            Err(e) => Err(DispatchErr::Fault(e.to_string())),
                        }
                    }
                }
            }
        }
    }

    /// True when the receiver's class chain contains a framework shim: a
    /// missing method there is an unimplemented API, not a resolution bug.
    fn chain_has_shim(&self, class_id: u32) -> bool {
        let mut cur = Some(class_id);
        while let Some(c) = cur {
            if matches!(self.registry.classes[c as usize].kind, crate::classes::ClassKind::Shim) {
                return true;
            }
            cur = self.registry.classes[c as usize].super_id;
        }
        false
    }

    fn init_class_for(&mut self, type_id: u32) -> Result<u32, DispatchErr> {
        let cid = self
            .registry
            .ensure_class(type_id)
            .map_err(|e| DispatchErr::Fault(e.to_string()))?;
        self.init_class(cid).map_err(|e| match e {
            VmFault::Java(j) => DispatchErr::Java(j),
            VmFault::Vm(m) => DispatchErr::Fault(m),
        })?;
        Ok(cid)
    }

    fn const_class_obj(&mut self, class_type: u32) -> Result<Value, DispatchErr> {
        let cid = self
            .registry
            .ensure_class(class_type)
            .map_err(|e| DispatchErr::Fault(e.to_string()))?;
        let r = self.heap.alloc(ObjectData::Class(cid), 48);
        Ok(Value::R(r))
    }

    /// Resolve a field reference (cached in the registry).
    fn field_ref_target(&mut self, field: &FieldRef) -> Result<crate::classes::FieldTarget, VmFault> {
        match field {
            FieldRef::Unresolved { class, name, desc } => {
                self.registry.resolve_field(*class, *name, *desc)
            }
            FieldRef::Resolved(ft) => Ok(*ft),
        }
    }
}



// ---------- math helpers ----------

fn math_i(op: MathOp, a: i32, b: i32) -> (Value, Option<ThrowSpec>) {
    let v = match op {
        MathOp::Add => Value::I(a.wrapping_add(b)),
        MathOp::Sub => Value::I(a.wrapping_sub(b)),
        MathOp::Mul => Value::I(a.wrapping_mul(b)),
        MathOp::Div => {
            if b == 0 {
                return (NULL, Some(ThrowSpec::new("Ljava/lang/ArithmeticException;", "divide by zero")));
            }
            Value::I(if a == i32::MIN && b == -1 { a } else { a.wrapping_div(b) })
        }
        MathOp::Rem => {
            if b == 0 {
                return (NULL, Some(ThrowSpec::new("Ljava/lang/ArithmeticException;", "divide by zero")));
            }
            Value::I(if a == i32::MIN && b == -1 { 0 } else { a.wrapping_rem(b) })
        }
        MathOp::And => Value::I(a & b),
        MathOp::Or => Value::I(a | b),
        MathOp::Xor => Value::I(a ^ b),
        MathOp::Shl => Value::I(a.wrapping_shl(b as u32)),
        MathOp::Shr => Value::I(a.wrapping_shr(b as u32)),
        MathOp::Ushr => Value::I(((a as u32).wrapping_shr(b as u32)) as i32),
    };
    (v, None)
}

fn math_l(op: MathOp, a: i64, b: i64) -> (Value, Option<ThrowSpec>) {
    let v = match op {
        MathOp::Add => Value::L(a.wrapping_add(b)),
        MathOp::Sub => Value::L(a.wrapping_sub(b)),
        MathOp::Mul => Value::L(a.wrapping_mul(b)),
        MathOp::Div => {
            if b == 0 {
                return (NULL, Some(ThrowSpec::new("Ljava/lang/ArithmeticException;", "divide by zero")));
            }
            Value::L(if a == i64::MIN && b == -1 { a } else { a.wrapping_div(b) })
        }
        MathOp::Rem => {
            if b == 0 {
                return (NULL, Some(ThrowSpec::new("Ljava/lang/ArithmeticException;", "divide by zero")));
            }
            Value::L(if a == i64::MIN && b == -1 { 0 } else { a.wrapping_rem(b) })
        }
        MathOp::And => Value::L(a & b),
        MathOp::Or => Value::L(a | b),
        MathOp::Xor => Value::L(a ^ b),
        MathOp::Shl => Value::L(a.wrapping_shl(b as u32)),
        MathOp::Shr => Value::L(a.wrapping_shr(b as u32)),
        MathOp::Ushr => Value::L(((a as u64).wrapping_shr(b as u32)) as i64),
    };
    (v, None)
}

/// Dalvik registers are untyped bit patterns; a `Value::L` reaching a double
/// op is a wide literal whose high half was set by const-wide/high16, so it
/// reinterprets as raw f64 bits.
fn as_double(v: Value) -> f64 {
    match v {
        Value::L(bits) => f64::from_bits(bits as u64),
        other => other.as_f64(),
    }
}

fn as_float(v: Value) -> f32 {
    match v {
        Value::I(bits) => f32::from_bits(bits as u32),
        other => other.as_f32(),
    }
}

fn math_f(op: MathOp, a: f32, b: f32) -> (Value, Option<ThrowSpec>) {
    let v = match op {
        MathOp::Add => Value::F(a + b),
        MathOp::Sub => Value::F(a - b),
        MathOp::Mul => Value::F(a * b),
        MathOp::Div => Value::F(a / b),
        MathOp::Rem => Value::F(java_frem(a, b)),
        _ => Value::F(0.0),
    };
    (v, None)
}

fn math_d(op: MathOp, a: f64, b: f64) -> (Value, Option<ThrowSpec>) {
    let v = match op {
        MathOp::Add => Value::D(a + b),
        MathOp::Sub => Value::D(a - b),
        MathOp::Mul => Value::D(a * b),
        MathOp::Div => Value::D(a / b),
        MathOp::Rem => Value::D(java_drem(a, b)),
        _ => Value::D(0.0),
    };
    (v, None)
}

/// Java's float remainder (fmod semantics like C fmodf).
fn java_frem(a: f32, b: f32) -> f32 {
    if a.is_nan() || b.is_nan() || a.is_infinite() || b == 0.0 {
        return f32::NAN;
    }
    if a == 0.0 || b.is_infinite() {
        return a;
    }
    let r = a % b; // Rust % matches fmod
    if r == 0.0 && a < 0.0 {
        -0.0
    } else {
        r
    }
}

fn java_drem(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() || a.is_infinite() || b == 0.0 {
        return f64::NAN;
    }
    if a == 0.0 || b.is_infinite() {
        return a;
    }
    let r = a % b;
    if r == 0.0 && a < 0.0 {
        -0.0
    } else {
        r
    }
}

fn cmp_f(a: f32, b: f32, nan_side: i32) -> i32 {
    if a.is_nan() || b.is_nan() {
        return nan_side;
    }
    if a < b {
        -1
    } else if a > b {
        1
    } else {
        0
    }
}

fn cmp_d(a: f64, b: f64, nan_side: i32) -> i32 {
    if a.is_nan() || b.is_nan() {
        return nan_side;
    }
    if a < b {
        -1
    } else if a > b {
        1
    } else {
        0
    }
}

fn convert(op: ConvOp, v: Value) -> Value {
    match op {
        ConvOp::IntToLong => Value::L(v.as_i32() as i64),
        ConvOp::IntToFloat => Value::F((v.as_i32() as f64) as f32),
        ConvOp::IntToDouble => Value::D(v.as_i32() as f64),
        ConvOp::LongToInt => Value::I(v.as_i64() as i32),
        ConvOp::LongToFloat => Value::F(v.as_i64() as f32),
        ConvOp::LongToDouble => Value::D(v.as_i64() as f64),
        ConvOp::FloatToInt => Value::I(v.as_f32() as i32),
        ConvOp::FloatToLong => Value::L(v.as_f32() as i64),
        ConvOp::FloatToDouble => Value::D(v.as_f32() as f64),
        ConvOp::DoubleToInt => Value::I(v.as_f64() as i32),
        ConvOp::DoubleToLong => Value::L(v.as_f64() as i64),
        ConvOp::DoubleToFloat => Value::F(v.as_f64() as f32),
        ConvOp::IntToByte => Value::I(v.as_i32() as i8 as i32),
        ConvOp::IntToChar => Value::I(v.as_i32() as u16 as i32),
        ConvOp::IntToShort => Value::I(v.as_i32() as i16 as i32),
        ConvOp::NegInt => Value::I(v.as_i32().wrapping_neg()),
        ConvOp::NotInt => Value::I(!v.as_i32()),
        ConvOp::NegLong => Value::L(v.as_i64().wrapping_neg()),
        ConvOp::NotLong => Value::L(!v.as_i64()),
        ConvOp::NegFloat => Value::F(-v.as_f32()),
        ConvOp::NegDouble => Value::D(-v.as_f64()),
    }
}

fn fill_into(dst: &mut [Value], kind: ArrayKind, width: u16, data: &[u8]) -> bool {
    let need = data.len() / width as usize;
    if dst.len() < need {
        return false;
    }
    let mut k = 0usize;
    for slot in dst.iter_mut().take(need) {
        let v = match (width, kind) {
            (1, _) => Value::I(data[k] as i32),
            (2, _) => {
                let lo = data[k] as u32;
                let hi = data[k + 1] as u32;
                Value::I((lo | (hi << 8)) as u16 as i32)
            }
            (4, ArrayKind::Floats) => {
                let b: [u8; 4] = data[k..k + 4].try_into().unwrap();
                Value::F(f32::from_le_bytes(b))
            }
            (4, _) => {
                let b: [u8; 4] = data[k..k + 4].try_into().unwrap();
                Value::I(i32::from_le_bytes(b))
            }
            (8, ArrayKind::Doubles) => {
                let b: [u8; 8] = data[k..k + 8].try_into().unwrap();
                Value::D(f64::from_le_bytes(b))
            }
            (8, _) => {
                let b: [u8; 8] = data[k..k + 8].try_into().unwrap();
                Value::L(i64::from_le_bytes(b))
            }
            _ => return false,
        };
        *slot = v;
        k += width as usize;
    }
    true
}

fn array_kind_of_desc(vm: &Machine, desc_id: u32) -> ArrayKind {
    let desc = vm.registry.type_str(desc_id);
    match desc.as_bytes().last() {
        Some(b'J') => ArrayKind::Longs,
        Some(b'F') => ArrayKind::Floats,
        Some(b'D') => ArrayKind::Doubles,
        Some(b'I') | Some(b'Z') | Some(b'B') | Some(b'C') | Some(b'S') => ArrayKind::Ints,
        _ => ArrayKind::Refs,
    }
}

fn first_byte_desc(vm: &Machine, desc_id: u32) -> Option<u8> {
    let desc = vm.registry.type_str(desc_id);
    desc.as_bytes().first().copied().filter(|&b| matches!(b, b'Z' | b'B' | b'C' | b'S'))
}

fn field_desc_str(vm: &Machine, f: &FieldRef) -> String {
    match f {
        FieldRef::Unresolved { class, name, desc } => format!(
            "{}.{}:{}",
            vm.registry.type_str(*class),
            vm.registry.name_str(*name),
            vm.registry.type_str(*desc)
        ),
        FieldRef::Resolved(ft) => match ft {
            crate::classes::FieldTarget::Instance { class_id, slot } => {
                format!("instance field @{}#{}", vm.registry.class_name(*class_id), slot)
            }
            crate::classes::FieldTarget::Static { class_id, index } => {
                format!("static field @{}#{}", vm.registry.class_name(*class_id), index)
            }
        },
    }
}

/// Patch a decoded instruction's direct-target cache in place (single
/// threaded VM, MethodCode is immutable after load except for this cache).
fn patch_direct_resolved(code: &Arc<MethodCode>, pc: usize, gidx: u32) {
    let ptr = Arc::as_ptr(code) as *mut MethodCode;
    // SAFETY: single-threaded VM; the Arc keeps the MethodCode alive and this
    // is the only writer (an inline cache fill on first execution).
    unsafe {
        let code_ref: &mut MethodCode = &mut *ptr;
        if let Some(Ins::Invoke { method: MethodRef::Direct { resolved, .. }, .. }) =
            code_ref.insns.get_mut(pc)
        {
            *resolved = gidx;
        }
    }
}

pub fn shorty_slots_of(shorty: &str) -> u8 {
    shorty
        .chars()
        .skip(1)
        .map(|c| if c == 'J' || c == 'D' { 2 } else { 1 })
        .sum()
}

/// Parse a method signature "(IJLjava/lang/String;)V" into per-slot widths.
/// Accepts both dex shorty ("VIJ") and full signature forms.
pub fn parse_sig(sig: &str) -> Vec<u8> {
    let inner = if let Some(open) = sig.find('(') {
        let close = sig.find(')').unwrap_or(sig.len());
        sig[open + 1..close].to_string()
    } else {
        // shorty form: skip return char
        sig.chars().skip(1).collect::<String>()
    };
    let mut out = Vec::new();
    let chars: Vec<char> = inner.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            'J' | 'D' => {
                out.push(2);
                i += 1;
            }
            'I' | 'Z' | 'B' | 'C' | 'S' | 'F' => {
                out.push(1);
                i += 1;
            }
            'L' | '[' => {
                out.push(1);
                while i < chars.len() && chars[i] != ';' {
                    i += 1;
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    out
}

/// shorty-style compression of a full signature (for vmap keys).
pub fn sig_to_shorty(sig: &str) -> String {
    let mut out = String::new();
    if let Some(open) = sig.find('(') {
        let close = sig.find(')').unwrap_or(sig.len());
        let ret = &sig[close + 1..];
        out.push(ret.chars().next().map(|c| match c {
            'J' => 'J',
            'D' => 'D',
            'V' => 'V',
            'L' => 'L',
            '[' => 'L',
            'I' | 'Z' | 'B' | 'C' | 'S' | 'F' => c,
            _ => 'I',
        }).unwrap_or('L'));
        let chars: Vec<char> = sig[open + 1..close].chars().collect();
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            match c {
                'J' | 'D' | 'I' | 'Z' | 'B' | 'C' | 'S' | 'F' => {
                    out.push(c);
                    i += 1;
                }
                'L' | '[' => {
                    out.push('L');
                    while i < chars.len() && chars[i] != ';' {
                        i += 1;
                    }
                    i += 1;
                }
                _ => i += 1,
            }
        }
    } else {
        out.push_str(sig);
    }
    out
}
