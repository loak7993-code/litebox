//! The VM service: one process owns one Machine, the package manager, and
//! the app handles. Every agent operation (JSON-RPC method, MCP tool, CLI
//! verb) maps to a method here, so all three control surfaces behave
//! identically.

use litebox_android::pm::{load_into, PackageManager};
use litebox_android::render_screen;
use litebox_dalvik::interp::{RunOutcome, VmStats};
use litebox_dalvik::snapshot;
use litebox_dalvik::value::Value;
use litebox_dalvik::{Machine, PauseReason};
use litebox_core::{Config, EventLevel};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

pub struct VmService {
    pub cfg: Config,
    pub machine: Machine,
    pub pm: PackageManager,
    pub apps: HashMap<String, litebox_android::pm::AppHandle>,
    seq: AtomicU32,
}

#[derive(Debug)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
}

impl RpcError {
    pub fn new(code: i64, msg: impl Into<String>) -> Self {
        RpcError { code, message: msg.into() }
    }
    pub fn invalid(msg: impl Into<String>) -> Self {
        Self::new(-32602, msg)
    }
    pub fn internal(msg: impl Into<String>) -> Self {
        Self::new(-32603, msg)
    }
}

type Res = Result<litebox_json::Json, RpcError>;

fn event_json(e: &litebox_core::EventRecord) -> litebox_json::Json {
    litebox_json::Json::obj(vec![
        ("seq", litebox_json::Json::u32n(e.seq as u32)),
        ("ts_us", litebox_json::Json::u32n((e.ts_us & 0xFFFF_FFFF) as u32)),
        ("level", litebox_json::Json::str(e.level.as_str())),
        ("tag", litebox_json::Json::str(e.tag.clone())),
        ("msg", litebox_json::Json::str(e.msg.clone())),
        ("data", e.data.clone().unwrap_or(litebox_json::Json::Null)),
    ])
}

fn value_to_json(m: &Machine, v: &Value) -> litebox_json::Json {
    match v {
        Value::I(x) => litebox_json::Json::num(*x as f64),
        Value::L(x) => litebox_json::Json::num(*x as f64),
        Value::F(x) => litebox_json::Json::num(*x as f64),
        Value::D(x) => litebox_json::Json::num(*x),
        Value::R(r) => match m.heap.get(*r) {
            Some(litebox_dalvik::ObjectData::Str(s)) => litebox_json::Json::str(s.clone()),
            Some(litebox_dalvik::ObjectData::Instance { class, .. }) => litebox_json::Json::obj(vec![
                ("ref", litebox_json::Json::num(r.idx as f64)),
                ("type", litebox_json::Json::str(m.registry.class_name(*class))),
            ]),
            Some(litebox_dalvik::ObjectData::Array { kind, data }) => {
                let show = data.iter().take(32).map(|x| value_to_json(m, x)).collect();
                litebox_json::Json::obj(vec![
                    ("ref", litebox_json::Json::num(r.idx as f64)),
                    ("kind", litebox_json::Json::str(format!("{kind:?}"))),
                    ("length", litebox_json::Json::num(data.len() as f64)),
                    ("items", litebox_json::Json::Arr(show)),
                ])
            }
            Some(litebox_dalvik::ObjectData::Class(c)) => litebox_json::Json::obj(vec![
                ("ref", litebox_json::Json::num(r.idx as f64)),
                ("class", litebox_json::Json::str(m.registry.class_name(*c))),
            ]),
            None => litebox_json::Json::Null,
        },
        Value::U => litebox_json::Json::Null,
    }
}

impl VmService {
    pub fn new(cfg: Config) -> Result<Self, String> {
        cfg.ensure_dirs().map_err(|e| format!("home dir: {e}"))?;
        let mut machine = Machine::new(&cfg);
        litebox_android::boot(&mut machine);
        let pm = PackageManager::new(cfg.packages_dir());
        Ok(VmService { cfg, machine, pm, apps: HashMap::new(), seq: AtomicU32::new(0) })
    }

    fn next_id(&self) -> u32 {
        self.seq.fetch_add(1, Ordering::SeqCst)
    }

    fn load_package(&mut self, pkg: &litebox_android::pm::InstalledPackage) -> Result<(), RpcError> {
        load_into(&mut self.machine, pkg).map_err(|e| RpcError::internal(format!("dex load: {e}")))?;
        Ok(())
    }

    /// Advance the VM until a pause or settle.
    fn run_budget(&mut self, budget: Option<u64>) -> litebox_json::Json {
        let outcome = match budget {
            Some(b) => {
                let task = self.machine.tasks.current_hint;
                self.machine.run_slice(task, b)
            }
            None => self.machine.run_until_pause(),
        };
        self.outcome_json(outcome)
    }

    fn outcome_json(&self, outcome: RunOutcome) -> litebox_json::Json {
        match outcome {
            RunOutcome::Paused(reason) => {
                let (kind, detail) = pause_reason_json(&reason);
                litebox_json::Json::obj(vec![
                    ("state", litebox_json::Json::str("paused")),
                    ("reason", litebox_json::Json::str(kind)),
                    ("detail", litebox_json::Json::str(detail)),
                ])
            }
            RunOutcome::Settled { alive, blocked } => litebox_json::Json::obj(vec![
                ("state", litebox_json::Json::str("settled")),
                ("alive", litebox_json::Json::num(alive as f64)),
                ("blocked", litebox_json::Json::num(blocked as f64)),
            ]),
            RunOutcome::Faulted(msg, task) => litebox_json::Json::obj(vec![
                ("state", litebox_json::Json::str("faulted")),
                ("task", litebox_json::Json::num(task as f64)),
                ("detail", litebox_json::Json::str(msg)),
            ]),
        }
    }

    // ---------- the dispatcher ----------

    pub fn dispatch(&mut self, method: &str, params: &litebox_json::Json) -> Res {
        match method {
            "vm.status" => self.vm_status(),
            "vm.reset" => self.vm_reset(),
            "pm.install" => self.pm_install(params),
            "pm.list" => self.pm_list(),
            "pm.uninstall" => self.pm_uninstall(params),
            "app.start" => self.app_start(params),
            "app.stop" => self.app_stop(params),
            "app.list" => self.app_list(),
            "run" => {
                self.pump_all();
                Ok(self.run_budget(param_u64(params, "budget")))
            }
            "resume" => {
                self.pump_all();
                self.machine.debug.paused = false;
                Ok(self.run_budget(param_u64(params, "budget")))
            }
            "step" => {
                let n = param_u64(params, "n").unwrap_or(1).max(1);
                self.machine.debug.paused = false;
                self.machine.debug.step_remaining = n;
                Ok(self.run_budget(Some(n)))
            }
            "pause" => {
                self.machine.debug.paused = true;
                self.machine.debug.last_reason = Some(PauseReason::Manual);
                Ok(litebox_json::Json::obj(vec![("state", litebox_json::Json::str("paused"))]))
            }
            "breakpoints.set" => self.bp_set(params),
            "breakpoints.list" => self.bp_list(),
            "breakpoints.clear" => self.bp_clear(),
            "state.tasks" => self.state_tasks(),
            "state.frames" => self.state_frames(params),
            "state.heap" => self.state_heap(),
            "state.objects" => self.state_objects(params),
            "state.object" => self.state_object(params),
            "state.statics" => self.state_statics(params),
            "object.write" => self.object_write(params),
            "static.write" => self.static_write(params),
            "call" => self.call(params),
            "screen.png" => self.screen_png(params),
            "screen.info" => self.screen_info(),
            "logcat" => self.logcat(params),
            "snapshot.save" => self.snapshot_save(params),
            "snapshot.load" => self.snapshot_load(params),
            "api.coverage" => self.api_coverage(),
            "adb.devices" => self.adb_devices(),
            "adb.connect" => self.adb_connect(params),
            "adb.shell" => self.adb_shell(params),
            "adb.install" => self.adb_install(params),
            "adb.uninstall" => self.adb_uninstall(params),
            "adb.am_start" => self.adb_am_start(params),
            "adb.input" => self.adb_input(params),
            "adb.screencap" => self.adb_screencap(params),
            "adb.logcat" => self.adb_logcat(params),
            "adb.packages" => self.adb_packages(params),
            "adb.ui" => self.adb_ui(params),
            _ => Err(RpcError::new(-32601, format!("unknown method: {method}"))),
        }
    }

    fn vm_status(&mut self) -> Res {
        let m = &self.machine;
        let apps: Vec<litebox_json::Json> = self
            .apps
            .iter()
            .map(|(pkg, h)| {
                litebox_json::Json::obj(vec![
                    ("package", litebox_json::Json::str(pkg.clone())),
                    ("task", litebox_json::Json::num(h.task as f64)),
                    ("class", litebox_json::Json::str(h.class_desc.clone())),
                    ("phase", litebox_json::Json::num(h.phase as f64)),
                ])
            })
            .collect();
        Ok(litebox_json::Json::obj(vec![
            ("paused", litebox_json::Json::Bool(m.debug.paused)),
            ("last_pause", m.debug.last_reason.clone().map(|r| {
                let (k, d): (&str, String) = match &r {
                    PauseReason::Manual => ("manual", String::new()),
                    PauseReason::Step => ("step", String::new()),
                    PauseReason::Breakpoint { method, pc } => ("breakpoint", format!("m{} pc{}", method, pc)),
                    PauseReason::MethodEntry { method } => ("method_entry", format!("m{}", method)),
                    PauseReason::Throw { class_desc, message } => ("throw", format!("{class_desc}: {message}")),
                    PauseReason::UnsupportedApi { detail } => ("unsupported_api", detail.clone()),
                    PauseReason::TaskExit { task } => ("task_exit", format!("t{}", task)),
                    PauseReason::Uncaught { class_desc, message, task } => {
                        ("uncaught", format!("t{task} {class_desc}: {message}"))
                    }
                    PauseReason::VmFault { detail } => ("vm_fault", detail.clone()),
                    PauseReason::Budget => ("budget", String::new()),
                    PauseReason::Idle => ("idle", String::new()),
                };
                litebox_json::Json::obj(vec![
                    ("reason", litebox_json::Json::str(k)),
                    ("detail", litebox_json::Json::str(d)),
                ])
            }).unwrap_or(litebox_json::Json::Null)),
            ("instructions", litebox_json::Json::num(m.debug.instruction_count as f64)),
            ("tasks", litebox_json::Json::num(m.tasks.live_count() as f64)),
            ("heap_objects", litebox_json::Json::num(m.heap.len() as f64)),
            ("heap_bytes", litebox_json::Json::num(m.heap.live_bytes as f64)),
            ("gcs", litebox_json::Json::num(m.heap.gc_count as f64)),
            ("uptime_ms", litebox_json::Json::num(m.clock.millis() as f64)),
            ("apps", litebox_json::Json::Arr(apps)),
            ("stats", self.stats_json()),
        ]))
    }

    fn stats_json(&self) -> litebox_json::Json {
        let s: &VmStats = &self.machine.stats;
        litebox_json::Json::obj(vec![
            ("instructions", litebox_json::Json::num(s.instructions as f64)),
            ("invokes", litebox_json::Json::num(s.invokes as f64)),
            ("native_calls", litebox_json::Json::num(s.native_calls as f64)),
            ("exceptions", litebox_json::Json::num(s.exceptions_thrown as f64)),
            ("allocations", litebox_json::Json::num(s.allocations as f64)),
            ("class_inits", litebox_json::Json::num(s.class_inits as f64)),
            ("task_switches", litebox_json::Json::num(s.task_switches as f64)),
            ("unsupported_calls", litebox_json::Json::num(s.unsupported_calls as f64)),
        ])
    }

    fn vm_reset(&mut self) -> Res {
        let cfg = self.cfg.clone();
        let svc = VmService::new(cfg).map_err(RpcError::internal)?;
        *self = svc;
        Ok(litebox_json::Json::obj(vec![("reset", litebox_json::Json::Bool(true))]))
    }

    fn pm_install(&mut self, params: &litebox_json::Json) -> Res {
        let path = params.str_field("path").ok_or_else(|| RpcError::invalid("path required"))?;
        let bytes = std::fs::read(path).map_err(|e| RpcError::invalid(format!("read {path}: {e}")))?;
        let now = self.machine.wall_clock_start_ms;
        let pkg = self
            .pm
            .install_bytes(&bytes, path.to_string(), now)
            .map_err(|e| RpcError::invalid(format!("install: {e}")))?;
        self.load_package(&pkg)?;
        Ok(litebox_json::Json::obj(vec![
            ("package", litebox_json::Json::str(pkg.package)),
            ("version", pkg.version_name.clone().map(litebox_json::Json::Str).unwrap_or(litebox_json::Json::Null)),
            ("label", pkg.label.clone().map(litebox_json::Json::Str).unwrap_or(litebox_json::Json::Null)),
            (
                "launcher",
                pkg.launcher_activity.clone().map(litebox_json::Json::Str).unwrap_or(litebox_json::Json::Null),
            ),
            ("dex_files", litebox_json::Json::num(pkg.dex_paths.len() as f64)),
        ]))
    }

    fn pm_list(&mut self) -> Res {
        let pkgs = self.pm.list();
        let arr = pkgs
            .into_iter()
            .map(|p| {
                litebox_json::Json::obj(vec![
                    ("package", litebox_json::Json::str(p.package)),
                    ("version", p.version_name.clone().map(litebox_json::Json::Str).unwrap_or(litebox_json::Json::Null)),
                    ("launcher", p.launcher_activity.clone().map(litebox_json::Json::Str).unwrap_or(litebox_json::Json::Null)),
                    ("activities", litebox_json::Json::Arr(p.activities.iter().map(|a| litebox_json::Json::str(a.clone())).collect())),
                ])
            })
            .collect();
        Ok(litebox_json::Json::Arr(arr))
    }

    fn pm_uninstall(&mut self, params: &litebox_json::Json) -> Res {
        let package = params.str_field("package").ok_or_else(|| RpcError::invalid("package required"))?;
        self.apps.remove(package);
        let existed = self.pm.uninstall(package).map_err(|e| RpcError::internal(e.to_string()))?;
        Ok(litebox_json::Json::obj(vec![("uninstalled", litebox_json::Json::Bool(existed))]))
    }

    fn app_start(&mut self, params: &litebox_json::Json) -> Res {
        let package = params.str_field("package").ok_or_else(|| RpcError::invalid("package required"))?;
        let pkgs = self.pm.list();
        let pkg = pkgs
            .into_iter()
            .find(|p| p.package == package)
            .ok_or_else(|| RpcError::invalid(format!("package {package} not installed")))?;
        self.load_package(&pkg)?;
        let activity = params.str_field("activity");
        let handle = litebox_android::pm::start_activity(&mut self.machine, &pkg, activity)
            .map_err(|e| RpcError::internal(format!("start: {e}")))?;
        let key = format!("{}#{}", package, self.next_id());
        let out = litebox_json::Json::obj(vec![
            ("handle", litebox_json::Json::str(key.clone())),
            ("task", litebox_json::Json::num(handle.task as f64)),
            ("class", litebox_json::Json::str(handle.class_desc.clone())),
        ]);
        let h2 = handle.clone();
        self.apps.insert(key, h2);
        // pump the first phase and run to the first pause
        let _ = handle.pump(&mut self.machine);
        let _ = self.run_budget(None);
        Ok(out)
    }

    fn app_stop(&mut self, params: &litebox_json::Json) -> Res {
        let package = params.str_field("handle").or_else(|| params.str_field("package"));
        let Some(package) = package else {
            return Err(RpcError::invalid("handle or package required"));
        };
        let removed: Vec<String> = self
            .apps
            .keys()
            .filter(|k| k.starts_with(package) || k.split('#').next() == Some(package))
            .cloned()
            .collect();
        for k in &removed {
            self.apps.remove(k);
        }
        Ok(litebox_json::Json::obj(vec![("stopped", litebox_json::Json::num(removed.len() as f64))]))
    }

    fn app_list(&mut self) -> Res {
        let arr = self
            .apps
            .iter()
            .map(|(k, h)| {
                litebox_json::Json::obj(vec![
                    ("handle", litebox_json::Json::str(k.clone())),
                    ("task", litebox_json::Json::num(h.task as f64)),
                    ("class", litebox_json::Json::str(h.class_desc.clone())),
                    ("phase", litebox_json::Json::num(h.phase as f64)),
                ])
            })
            .collect();
        Ok(litebox_json::Json::Arr(arr))
    }

    /// Advance all app handles that finished their previous phase.
    fn pump_all(&mut self) {
        let handles: Vec<String> = self.apps.keys().cloned().collect();
        for k in handles {
            if let Some(h) = self.apps.get(&k).cloned() {
                if let Ok(true) = h.pump(&mut self.machine) {
                    if let Some(h) = self.apps.get_mut(&k) {
                        h.phase += 1;
                    }
                }
            }
        }
    }

    fn bp_set(&mut self, params: &litebox_json::Json) -> Res {
        if let (Some(cls), Some(meth)) = (params.str_field("class"), params.str_field("method")) {
            self.machine.debug.named_breakpoints.insert((cls.to_string(), meth.to_string()));
            // also try to bind to a global method index now
            if let Some(cid) = self.machine.registry.class_by_name(cls) {
                let nid = self.machine.registry.name_id(meth);
                if let Some(nid) = nid {
                    for &g in &self.machine.registry.classes[cid as usize].own_methods {
                        if self.machine.registry.method_at(g).map(|m| m.name) == Some(nid) {
                            self.machine.debug.method_breakpoints.insert(g);
                        }
                    }
                }
            }
            return Ok(litebox_json::Json::obj(vec![("bound", litebox_json::Json::Bool(true))]));
        }
        let method = param_u64(params, "method").ok_or_else(|| RpcError::invalid("method or class+method required"))?;
        let pc = param_u64(params, "pc").unwrap_or(0) as u32;
        self.machine.debug.breakpoints.insert((method as u32, pc));
        Ok(litebox_json::Json::obj(vec![("bound", litebox_json::Json::Bool(true))]))
    }

    fn bp_list(&mut self) -> Res {
        let m = &self.machine;
        let exact = m
            .debug
            .breakpoints
            .iter()
            .map(|(meth, pc)| {
                let name = m
                    .registry
                    .method_at(*meth)
                    .map(|e| {
                        format!(
                            "{}.{}",
                            m.registry.class_name(e.class_id),
                            m.registry.name_str(e.name)
                        )
                    })
                    .unwrap_or_else(|| format!("method#{}", meth));
                litebox_json::Json::obj(vec![
                    ("method", litebox_json::Json::num(*meth as f64)),
                    ("pc", litebox_json::Json::num(*pc as f64)),
                    ("name", litebox_json::Json::str(name)),
                ])
            })
            .collect::<Vec<_>>();
        let by_method = m
            .debug
            .method_breakpoints
            .iter()
            .map(|meth| {
                let name = m
                    .registry
                    .method_at(*meth)
                    .map(|e| {
                        format!(
                            "{}.{}",
                            m.registry.class_name(e.class_id),
                            m.registry.name_str(e.name)
                        )
                    })
                    .unwrap_or_else(|| format!("method#{}", meth));
                litebox_json::Json::obj(vec![
                    ("method", litebox_json::Json::num(*meth as f64)),
                    ("name", litebox_json::Json::str(name)),
                ])
            })
            .collect::<Vec<_>>();
        let named = m
            .debug
            .named_breakpoints
            .iter()
            .map(|(c, n)| litebox_json::Json::str(format!("{c}.{n}")))
            .collect::<Vec<_>>();
        Ok(litebox_json::Json::obj(vec![
            ("exact", litebox_json::Json::Arr(exact)),
            ("by_method", litebox_json::Json::Arr(by_method)),
            ("named", litebox_json::Json::Arr(named)),
        ]))
    }

    fn bp_clear(&mut self) -> Res {
        self.machine.debug.breakpoints.clear();
        self.machine.debug.method_breakpoints.clear();
        self.machine.debug.named_breakpoints.clear();
        Ok(litebox_json::Json::obj(vec![("cleared", litebox_json::Json::Bool(true))]))
    }

    fn state_tasks(&mut self) -> Res {
        let arr = self
            .machine
            .tasks
            .tasks
            .iter()
            .flatten()
            .map(|t| {
                litebox_json::Json::obj(vec![
                    ("id", litebox_json::Json::num(t.id as f64)),
                    ("name", litebox_json::Json::str(t.name.clone())),
                    ("state", litebox_json::Json::str(format!("{:?}", t.state).split('{').next().unwrap_or("?").trim().to_string())),
                    ("depth", litebox_json::Json::num(t.stack.len() as f64)),
                    (
                        "frame",
                        t.stack.last().map(|f| {
                            litebox_json::Json::num(f.method as f64)
                        }).unwrap_or(litebox_json::Json::Null),
                    ),
                ])
            })
            .collect();
        Ok(litebox_json::Json::Arr(arr))
    }

    fn state_frames(&mut self, params: &litebox_json::Json) -> Res {
        let task = param_u64(params, "task").unwrap_or(self.machine.tasks.current_hint as u64) as u32;
        let t = self
            .machine
            .tasks
            .task(task)
            .ok_or_else(|| RpcError::invalid(format!("task {task} not found")))?;
        let frames = t
            .stack
            .iter()
            .enumerate()
            .rev()
            .map(|(i, f)| {
                let me = self.machine.registry.method_at(f.method);
                litebox_json::Json::obj(vec![
                    ("index", litebox_json::Json::num(i as f64)),
                    (
                        "class",
                        litebox_json::Json::str(self.machine.registry.class_name(f.class_id)),
                    ),
                    (
                        "method",
                        litebox_json::Json::str(
                            me.map(|m| self.machine.registry.name_str(m.name).to_string())
                                .unwrap_or_default(),
                        ),
                    ),
                    ("pc", litebox_json::Json::num(f.pc as f64)),
                    (
                        "method_global",
                        litebox_json::Json::num(f.method as f64),
                    ),
                ])
            })
            .collect();
        Ok(litebox_json::Json::Arr(frames))
    }

    fn state_heap(&mut self) -> Res {
        let m = &self.machine;
        let mut by_type: HashMap<String, u64> = HashMap::new();
        for (_, obj) in m.heap.iter_slots() {
            let key = match obj {
                litebox_dalvik::ObjectData::Instance { class, .. } => {
                    m.registry.class_name(*class).to_string()
                }
                litebox_dalvik::ObjectData::Array { .. } => "array".to_string(),
                litebox_dalvik::ObjectData::Str(_) => "string".to_string(),
                litebox_dalvik::ObjectData::Class(_) => "class".to_string(),
            };
            *by_type.entry(key).or_insert(0) += 1;
        }
        let breakdown = by_type
            .into_iter()
            .map(|(k, v)| litebox_json::Json::obj(vec![("type", litebox_json::Json::str(k)), ("count", litebox_json::Json::num(v as f64))]))
            .collect();
        Ok(litebox_json::Json::obj(vec![
            ("objects", litebox_json::Json::num(m.heap.len() as f64)),
            ("live_bytes", litebox_json::Json::num(m.heap.live_bytes as f64)),
            ("allocations", litebox_json::Json::num(m.heap.alloc_count as f64)),
            ("gcs", litebox_json::Json::num(m.heap.gc_count as f64)),
            ("budget_bytes", litebox_json::Json::num(m.heap_budget as f64)),
            ("by_type", litebox_json::Json::Arr(breakdown)),
        ]))
    }

    /// List live object refs, optionally filtered by class substring.
    fn state_objects(&mut self, params: &litebox_json::Json) -> Res {
        let class_filter = params.str_field("class").unwrap_or("").to_string();
        let limit = param_u64(params, "limit").unwrap_or(50).min(500) as usize;
        let mut out = Vec::new();
        for (idx, obj) in self.machine.heap.iter_slots() {
            let type_name = match obj {
                litebox_dalvik::ObjectData::Instance { class, .. } => {
                    self.machine.registry.class_name(*class).to_string()
                }
                litebox_dalvik::ObjectData::Str(_) => "java/lang/String".to_string(),
                litebox_dalvik::ObjectData::Array { .. } => "array".to_string(),
                litebox_dalvik::ObjectData::Class(_) => "java/lang/Class".to_string(),
            };
            if !class_filter.is_empty() && !type_name.contains(&class_filter) {
                continue;
            }
            let preview = match obj {
                litebox_dalvik::ObjectData::Str(s) => Some(s.to_string()),
                litebox_dalvik::ObjectData::Instance { .. } => self
                    .machine
                    .get_instance_field(litebox_dalvik::ObjRef { idx, gen: 0 }, "text")
                    .and_then(|v| self.machine.str_value(v)),
                _ => None,
            };
            out.push(litebox_json::Json::obj(vec![
                ("ref", litebox_json::Json::num(idx as f64)),
                ("type", litebox_json::Json::str(type_name)),
                (
                    "text",
                    preview.map(litebox_json::Json::Str).unwrap_or(litebox_json::Json::Null),
                ),
            ]));
            if out.len() >= limit {
                break;
            }
        }
        Ok(litebox_json::Json::Arr(out))
    }

    fn state_object(&mut self, params: &litebox_json::Json) -> Res {
        let id = param_u64(params, "ref").ok_or_else(|| RpcError::invalid("ref required"))? as u32;
        let r = litebox_dalvik::ObjRef { idx: id, gen: 0 };
        let obj = self
            .machine
            .heap
            .get(r)
            .ok_or_else(|| RpcError::invalid(format!("object {id} not live")))?;
        match obj {
            litebox_dalvik::ObjectData::Instance { class, fields } => {
                let cls = &self.machine.registry.classes[*class as usize];
                let list = cls
                    .instance_fields
                    .iter()
                    .enumerate()
                    .map(|(i, f)| {
                        litebox_json::Json::obj(vec![
                            ("name", litebox_json::Json::str(self.machine.registry.name_str(f.name))),
                            ("value", value_to_json(&self.machine, fields.get(i).unwrap_or(&Value::U))),
                        ])
                    })
                    .collect();
                Ok(litebox_json::Json::obj(vec![
                    ("ref", litebox_json::Json::num(id as f64)),
                    ("type", litebox_json::Json::str(self.machine.registry.class_name(*class))),
                    ("fields", litebox_json::Json::Arr(list)),
                ]))
            }
            _other => Ok(value_to_json(
                &self.machine,
                &Value::R(litebox_dalvik::ObjRef { idx: id, gen: 0 }),
            ))
        }
    }

    fn state_statics(&mut self, params: &litebox_json::Json) -> Res {
        let cls = params.str_field("class").ok_or_else(|| RpcError::invalid("class required"))?;
        let cid = self
            .machine
            .registry
            .class_by_name(cls)
            .ok_or_else(|| RpcError::invalid(format!("class {cls} not loaded")))?;
        let list = self.machine.registry.classes[cid as usize]
            .statics
            .iter()
            .map(|s| {
                litebox_json::Json::obj(vec![
                    ("name", litebox_json::Json::str(self.machine.registry.name_str(s.name))),
                    ("value", value_to_json(&self.machine, &s.value)),
                ])
            })
            .collect();
        Ok(litebox_json::Json::Arr(list))
    }

    fn object_write(&mut self, params: &litebox_json::Json) -> Res {
        let id = param_u64(params, "ref").ok_or_else(|| RpcError::invalid("ref required"))? as u32;
        let field = params.str_field("field").ok_or_else(|| RpcError::invalid("field required"))?;
        let v = params.get("value").ok_or_else(|| RpcError::invalid("value required"))?;
        let r = litebox_dalvik::ObjRef { idx: id, gen: 0 };
        if !self.machine.heap.is_live(r) {
            return Err(RpcError::invalid(format!("object {id} not live")));
        }
        let nv = json_to_value(self, v);
        let ok = self.machine.set_instance_field(r, field, nv);
        Ok(litebox_json::Json::obj(vec![("written", litebox_json::Json::Bool(ok))]))
    }

    fn static_write(&mut self, params: &litebox_json::Json) -> Res {
        let cls = params.str_field("class").ok_or_else(|| RpcError::invalid("class required"))?;
        let field = params.str_field("field").ok_or_else(|| RpcError::invalid("field required"))?;
        let v = params.get("value").ok_or_else(|| RpcError::invalid("value required"))?;
        let cid = self
            .machine
            .registry
            .class_by_name(cls)
            .ok_or_else(|| RpcError::invalid(format!("class {cls} not loaded")))?;
        let nid = self
            .machine
            .registry
            .name_id(field)
            .ok_or_else(|| RpcError::invalid(format!("field {field} unknown")))?;
        let nv = json_to_value(self, v);
        let mut written = false;
        for s in self.machine.registry.classes[cid as usize].statics.iter_mut() {
            if s.name == nid {
                s.value = nv;
                written = true;
                break;
            }
        }
        Ok(litebox_json::Json::obj(vec![("written", litebox_json::Json::Bool(written))]))
    }

    fn call(&mut self, params: &litebox_json::Json) -> Res {
        let class_desc = params.str_field("class").ok_or_else(|| RpcError::invalid("class required"))?;
        let method = params.str_field("method").ok_or_else(|| RpcError::invalid("method required"))?;
        let sig = params.str_field("sig").ok_or_else(|| RpcError::invalid("sig required"))?;
        let args_in: Vec<litebox_json::Json> = params
            .get("args")
            .and_then(|a| a.as_arr())
            .map(|a| a.to_vec())
            .unwrap_or_default();
        let this_ref = params.get("this").and_then(|t| t.as_u32());
        let this = match this_ref {
            Some(id) => {
                let r = litebox_dalvik::ObjRef { idx: id, gen: 0 };
                if !self.machine.heap.is_live(r) {
                    return Err(RpcError::invalid(format!("this {id} not live")));
                }
                Value::R(r)
            }
            None => Value::null(),
        };
        let mut args = Vec::with_capacity(args_in.len());
        for a in &args_in {
            args.push(json_to_value(self, a));
        }
        // agent calls honor breakpoints/steps: they can pause mid-call with
        // the frames intact for inspection
        match self
            .machine
            .call_method_debuggable(this, class_desc, method, sig, args)
        {
            Ok(litebox_dalvik::interp::CallOutcome::Returned(v)) => {
                Ok(litebox_json::Json::obj(vec![("returned", value_to_json(&self.machine, &v))]))
            }
            Ok(litebox_dalvik::interp::CallOutcome::Paused(reason)) => {
                let (kind, detail) = pause_reason_json(&reason);
                Ok(litebox_json::Json::obj(vec![
                    ("state", litebox_json::Json::str("paused")),
                    ("reason", litebox_json::Json::str(kind)),
                    ("detail", litebox_json::Json::str(detail)),
                ]))
            }
            Err(exn) => {
                let msg = self.machine.exn_message(&exn);
                Err(RpcError::new(
                    -32000,
                    format!("java exception: {} {}", exn.class_desc, msg),
                ))
            }
        }
    }

    fn screen_png(&mut self, params: &litebox_json::Json) -> Res {
        let _rendered = render_screen(&mut self.machine);
        let png = png_encode(&self.machine.screen);
        match params.str_field("out") {
            Some(out) => {
                std::fs::write(out, &png).map_err(|e| RpcError::internal(format!("write: {e}")))?;
                Ok(litebox_json::Json::obj(vec![
                    ("written", litebox_json::Json::str(out)),
                    ("bytes", litebox_json::Json::num(png.len() as f64)),
                ]))
            }
            None => Ok(litebox_json::Json::obj(vec![
                ("base64", litebox_json::Json::str(crate::util::base64(&png))),
                ("bytes", litebox_json::Json::num(png.len() as f64)),
            ])),
        }
    }

    fn screen_info(&mut self) -> Res {
        let s = &self.machine.screen;
        Ok(litebox_json::Json::obj(vec![
            ("width", litebox_json::Json::num(s.width as f64)),
            ("height", litebox_json::Json::num(s.height as f64)),
            ("version", litebox_json::Json::num(s.version as f64)),
        ]))
    }

    fn logcat(&mut self, params: &litebox_json::Json) -> Res {
        let cursor = param_u64(params, "cursor").unwrap_or(0);
        let level = params.str_field("level").and_then(EventLevel::parse);
        let tag_filter = params.str_field("tag");
        let (events, new_cursor) = self.machine.bus.take_after(cursor, 5000);
        let filtered: Vec<_> = events
            .into_iter()
            .filter(|e| level.map(|l| e.level >= l).unwrap_or(true))
            .filter(|e| tag_filter.as_ref().map(|t| e.tag.contains(t)).unwrap_or(true))
            .collect();
        let last = filtered.last().map(|e| e.seq + 1).unwrap_or(cursor);
        let _ = new_cursor;
        Ok(litebox_json::Json::obj(vec![
            ("cursor", litebox_json::Json::num(last as f64)),
            (
                "events",
                litebox_json::Json::Arr(filtered.iter().map(event_json).collect()),
            ),
        ]))
    }

    fn snapshot_save(&mut self, params: &litebox_json::Json) -> Res {
        let path = params
            .str_field("path")
            .map(PathBuf::from)
            .unwrap_or_else(|| self.cfg.snapshots_dir().join(format!("snap-{}.bin", self.next_id())));
        let blob = snapshot::save(&self.machine);
        std::fs::write(&path, &blob).map_err(|e| RpcError::internal(format!("write: {e}")))?;
        Ok(litebox_json::Json::obj(vec![
            ("path", litebox_json::Json::str(path.display().to_string())),
            ("bytes", litebox_json::Json::num(blob.len() as f64)),
        ]))
    }

    fn snapshot_load(&mut self, params: &litebox_json::Json) -> Res {
        let path = params.str_field("path").ok_or_else(|| RpcError::invalid("path required"))?;
        let blob = std::fs::read(path).map_err(|e| RpcError::invalid(format!("read: {e}")))?;
        snapshot::load(&mut self.machine, &blob).map_err(|e| RpcError::internal(e.to_string()))?;
        Ok(litebox_json::Json::obj(vec![
            ("loaded", litebox_json::Json::Bool(true)),
            ("bytes", litebox_json::Json::num(blob.len() as f64)),
        ]))
    }

    fn adb(&self) -> Result<PathBuf, RpcError> {
        crate::adb::find_adb().map_err(|e| RpcError::internal(e.0))
    }

    fn adb_devices(&mut self) -> Res {
        let adb = self.adb()?;
        crate::adb::devices(&adb).map_err(|e| RpcError::internal(e.0))
    }

    fn adb_connect(&mut self, params: &litebox_json::Json) -> Res {
        let adb = self.adb()?;
        let host = params.str_field("host").ok_or_else(|| RpcError::invalid("host:port required"))?;
        let (code, out, err) = crate::adb::run(
            &adb,
            &["connect".to_string(), host.to_string()],
            std::time::Duration::from_secs(15),
        )
        .map_err(|e| RpcError::internal(e.0))?;
        Ok(litebox_json::Json::obj(vec![
            ("code", litebox_json::Json::num(code as f64)),
            ("output", litebox_json::Json::str(format!("{}{}", out, err).trim().to_string())),
        ]))
    }

    fn adb_shell(&mut self, params: &litebox_json::Json) -> Res {
        let adb = self.adb()?;
        let cmd = params.str_field("cmd").ok_or_else(|| RpcError::invalid("cmd required"))?;
        let serial = params.str_field("serial");
        let timeout = std::time::Duration::from_secs(param_u64(params, "timeout").unwrap_or(30));
        crate::adb::shell(&adb, serial, cmd, timeout).map_err(|e| RpcError::internal(e.0))
    }

    fn adb_install(&mut self, params: &litebox_json::Json) -> Res {
        let adb = self.adb()?;
        let path = params.str_field("path").ok_or_else(|| RpcError::invalid("path required"))?;
        let serial = params.str_field("serial");
        let reinstall = params.get("reinstall").and_then(|v| v.as_bool()).unwrap_or(true);
        crate::adb::install(&adb, serial, path, reinstall).map_err(|e| RpcError::internal(e.0))
    }

    fn adb_uninstall(&mut self, params: &litebox_json::Json) -> Res {
        let adb = self.adb()?;
        let package = params.str_field("package").ok_or_else(|| RpcError::invalid("package required"))?;
        let serial = params.str_field("serial");
        crate::adb::uninstall(&adb, serial, package).map_err(|e| RpcError::internal(e.0))
    }

    fn adb_am_start(&mut self, params: &litebox_json::Json) -> Res {
        let adb = self.adb()?;
        let component = params.str_field("component").ok_or_else(|| RpcError::invalid("component (pkg/.Activity) required"))?;
        let serial = params.str_field("serial");
        crate::adb::am_start(&adb, serial, component).map_err(|e| RpcError::internal(e.0))
    }

    fn adb_input(&mut self, params: &litebox_json::Json) -> Res {
        let adb = self.adb()?;
        let kind = params.str_field("kind").ok_or_else(|| RpcError::invalid("kind required (tap|swipe|text|keyevent)"))?;
        let coords: Vec<i64> = params
            .get("args")
            .and_then(|a| a.as_arr())
            .map(|a| a.iter().filter_map(|v| v.as_i64()).collect())
            .unwrap_or_default();
        let serial = params.str_field("serial");
        crate::adb::input(&adb, serial, kind, &coords).map_err(|e| RpcError::internal(e.0))
    }

    fn adb_screencap(&mut self, params: &litebox_json::Json) -> Res {
        let adb = self.adb()?;
        let serial = params.str_field("serial");
        let png = crate::adb::screencap_png(&adb, serial).map_err(|e| RpcError::internal(e.0))?;
        match params.str_field("out") {
            Some(out) => {
                std::fs::write(out, &png).map_err(|e| RpcError::internal(format!("write: {e}")))?;
                Ok(litebox_json::Json::obj(vec![
                    ("written", litebox_json::Json::str(out)),
                    ("bytes", litebox_json::Json::num(png.len() as f64)),
                ]))
            }
            None => Ok(litebox_json::Json::obj(vec![
                ("base64", litebox_json::Json::str(crate::util::base64(&png))),
                ("bytes", litebox_json::Json::num(png.len() as f64)),
            ])),
        }
    }

    fn adb_logcat(&mut self, params: &litebox_json::Json) -> Res {
        let adb = self.adb()?;
        let serial = params.str_field("serial");
        let tail = param_u64(params, "tail").unwrap_or(200).min(5000) as usize;
        let filter = params.str_field("filter");
        crate::adb::logcat(&adb, serial, tail, filter).map_err(|e| RpcError::internal(e.0))
    }

    fn adb_packages(&mut self, params: &litebox_json::Json) -> Res {
        let adb = self.adb()?;
        let serial = params.str_field("serial");
        crate::adb::packages(&adb, serial).map_err(|e| RpcError::internal(e.0))
    }

    fn adb_ui(&mut self, params: &litebox_json::Json) -> Res {
        let adb = self.adb()?;
        let serial = params.str_field("serial");
        crate::adb::ui_dump(&adb, serial).map_err(|e| RpcError::internal(e.0))
    }

    fn api_coverage(&mut self) -> Res {
        let classes: Vec<litebox_json::Json> = self
            .machine
            .registry
            .classes
            .iter()
            .filter(|c| matches!(c.kind, litebox_dalvik::classes::ClassKind::Shim))
            .map(|c| {
                litebox_json::Json::obj(vec![
                    ("class", litebox_json::Json::str(c.name.clone())),
                    ("methods", litebox_json::Json::num(c.own_methods.len() as f64)),
                ])
            })
            .collect();
        Ok(litebox_json::Json::obj(vec![
            ("shim_classes", litebox_json::Json::Arr(classes)),
            ("dex_files", litebox_json::Json::num(self.machine.registry.dexes.len() as f64)),
            ("native_methods", litebox_json::Json::num(self.machine.registry.natives.len() as f64)),
            ("stats", self.stats_json()),
        ]))
    }
}

fn pause_reason_json(reason: &PauseReason) -> (&'static str, String) {
    match reason {
        PauseReason::Manual => ("manual", String::new()),
        PauseReason::Step => ("step", String::new()),
        PauseReason::Breakpoint { method, pc } => ("breakpoint", format!("method {method} pc {pc}")),
        PauseReason::MethodEntry { method } => ("method_entry", format!("method {method}")),
        PauseReason::Throw { class_desc, message } => ("throw", format!("{class_desc}: {message}")),
        PauseReason::UnsupportedApi { detail } => ("unsupported_api", detail.clone()),
        PauseReason::TaskExit { task } => ("task_exit", format!("task {task}")),
        PauseReason::Uncaught { class_desc, message, task } => {
            ("uncaught", format!("task {task}: {class_desc}: {message}"))
        }
        PauseReason::VmFault { detail } => ("vm_fault", detail.clone()),
        PauseReason::Budget => ("budget", String::new()),
        PauseReason::Idle => ("idle", String::new()),
    }
}

fn param_u64(params: &litebox_json::Json, key: &str) -> Option<u64> {
    params.get(key).and_then(|v| v.as_i64()).map(|v| v as u64)
}

fn json_to_value(svc: &mut VmService, v: &litebox_json::Json) -> Value {
    match v {
        litebox_json::Json::Null => Value::null(),
        litebox_json::Json::Bool(b) => Value::I(*b as i32),
        litebox_json::Json::Num(n) => {
            if n.fract() == 0.0 {
                if *n > i32::MAX as f64 || *n < i32::MIN as f64 {
                    Value::L(*n as i64)
                } else {
                    Value::I(*n as i32)
                }
            } else {
                Value::D(*n)
            }
        }
        litebox_json::Json::Str(s) => {
            // strings are interned objects
            svc.machine.make_str(s)
        }
        other => svc.machine.make_str(&other.to_string()),
    }
}

/// Minimal PNG encoder (8-bit RGBA, stored-deflate IDAT).
pub fn png_encode(screen: &litebox_core::Screen) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]);
    let chunk = |out: &mut Vec<u8>, typ: &[u8; 4], data: &[u8]| {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(typ);
        out.extend_from_slice(data);
        let mut crc_data = typ.to_vec();
        crc_data.extend_from_slice(data);
        out.extend_from_slice(&litebox_zip::crc32(&crc_data).to_be_bytes());
    };
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&screen.width.to_be_bytes());
    ihdr.extend_from_slice(&screen.height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    let mut raw = Vec::with_capacity(screen.height as usize * (1 + screen.width as usize * 4));
    for y in 0..screen.height as usize {
        raw.push(0);
        let start = y * screen.width as usize * 4;
        raw.extend_from_slice(&screen.buf[start..start + screen.width as usize * 4]);
    }
    chunk(&mut out, b"IDAT", &litebox_zip::inflate::zlib_compress_stored(&raw));
    chunk(&mut out, b"IEND", &[]);
    out
}

