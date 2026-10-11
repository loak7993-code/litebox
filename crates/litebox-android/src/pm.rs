//! Package manager: install APKs (extract dex + parse manifest), list,
//! uninstall, and launch the main activity into a booted VM.

use crate::axml;
use litebox_core::EventLevel;
use litebox_dalvik::tasks::TaskState;
use litebox_dalvik::Machine;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct InstalledPackage {
    pub package: String,
    pub version_name: Option<String>,
    pub label: Option<String>,
    pub launcher_activity: Option<String>,
    pub activities: Vec<String>,
    pub dex_paths: Vec<String>,
    pub installed_at_ms: u64,
    pub source: String,
}

#[derive(Debug)]
pub enum PmError {
    Io(std::io::Error),
    Zip(litebox_zip::ZipError),
    Dex(litebox_dex::DexError),
    Manifest(String),
    NoDex,
    NotFound(String),
}

impl std::fmt::Display for PmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PmError::Io(e) => write!(f, "io: {e}"),
            PmError::Zip(e) => write!(f, "apk: {e}"),
            PmError::Dex(e) => write!(f, "dex: {e}"),
            PmError::Manifest(m) => write!(f, "manifest: {m}"),
            PmError::NoDex => write!(f, "no classes.dex in package"),
            PmError::NotFound(p) => write!(f, "package not installed: {p}"),
        }
    }
}
impl std::error::Error for PmError {}

pub struct PackageManager {
    pub dir: std::path::PathBuf,
}

impl PackageManager {
    pub fn new(dir: std::path::PathBuf) -> Self {
        PackageManager { dir }
    }

    pub fn install_apk(&self, apk_path: &Path, now_ms: u64) -> Result<InstalledPackage, PmError> {
        let bytes = std::fs::read(apk_path).map_err(PmError::Io)?;
        self.install_bytes(&bytes, apk_path.display().to_string(), now_ms)
    }

    pub fn install_bytes(
        &self,
        bytes: &[u8],
        source: String,
        now_ms: u64,
    ) -> Result<InstalledPackage, PmError> {
        let ar = litebox_zip::ZipArchive::open(bytes).map_err(PmError::Zip)?;
        let axml = ar
            .read("AndroidManifest.xml")
            .map_err(|_| PmError::Manifest("missing AndroidManifest.xml".into()))?;
        let manifest = axml::parse_manifest(&axml).map_err(PmError::Manifest)?;
        if manifest.package.is_empty() {
            return Err(PmError::Manifest("manifest has no package attribute".into()));
        }
        // collect all dex files (classes.dex, classes2.dex, ...); each must
        // parse before the package is accepted
        let pkg_dir = self.dir.join(&manifest.package);
        std::fs::create_dir_all(&pkg_dir).map_err(PmError::Io)?;
        let mut dex_paths = Vec::new();
        let mut dex_count = 0;
        for name in ["classes.dex", "classes2.dex", "classes3.dex", "classes4.dex"] {
            if let Ok(dex) = ar.read(name) {
                litebox_dex::Dex::parse(dex.clone()).map_err(PmError::Dex)?;
                let disk = pkg_dir.join(name);
                std::fs::write(&disk, &dex).map_err(PmError::Io)?;
                dex_paths.push(disk.display().to_string());
                dex_count += 1;
            }
        }
        if dex_count == 0 {
            return Err(PmError::NoDex);
        }
        let pkg = InstalledPackage {
            package: manifest.package.clone(),
            version_name: manifest.version_name.clone(),
            label: manifest.label.clone(),
            launcher_activity: manifest.launcher_activity.clone(),
            activities: manifest.activities.clone(),
            dex_paths,
            installed_at_ms: now_ms,
            source,
        };
        let meta = litebox_json::Json::obj(vec![
            ("package", litebox_json::Json::str(pkg.package.clone())),
            (
                "version_name",
                pkg.version_name.clone().map(litebox_json::Json::Str).unwrap_or(litebox_json::Json::Null),
            ),
            (
                "label",
                pkg.label.clone().map(litebox_json::Json::Str).unwrap_or(litebox_json::Json::Null),
            ),
            (
                "launcher_activity",
                pkg.launcher_activity.clone().map(litebox_json::Json::Str).unwrap_or(litebox_json::Json::Null),
            ),
            (
                "activities",
                litebox_json::Json::Arr(pkg.activities.iter().map(|a| litebox_json::Json::str(a.clone())).collect()),
            ),
            (
                "dex_paths",
                litebox_json::Json::Arr(pkg.dex_paths.iter().map(|d| litebox_json::Json::str(d.clone())).collect()),
            ),
            ("installed_at_ms", litebox_json::Json::u32n(pkg.installed_at_ms as u32)),
            ("source", litebox_json::Json::str(pkg.source.clone())),
        ]);
        std::fs::write(
            self.dir.join(format!("{}.json", pkg.package)),
            meta.to_pretty(),
        )
        .map_err(PmError::Io)?;
        Ok(pkg)
    }

    pub fn list(&self) -> Vec<InstalledPackage> {
        let mut out = Vec::new();
        let Ok(entries) = std::fs::read_dir(&self.dir) else { return out };
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) == Some("json") {
                if let Ok(txt) = std::fs::read_to_string(&p) {
                    if let Ok(v) = litebox_json::Json::parse(&txt) {
                        let package = v.str_field("package").unwrap_or_default().to_string();
                        let dex_paths = v
                            .get("dex_paths")
                            .and_then(|d| d.as_arr())
                            .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
                            .unwrap_or_else(Vec::new);
                        let launcher_activity =
                            v.str_field("launcher_activity").map(String::from);
                        let activities = v
                            .get("activities")
                            .and_then(|d| d.as_arr())
                            .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
                            .unwrap_or_else(Vec::new);
                        out.push(InstalledPackage {
                            package,
                            version_name: v.str_field("version_name").map(String::from),
                            label: v.str_field("label").map(String::from),
                            launcher_activity,
                            activities,
                            dex_paths,
                            installed_at_ms: v.u32_field("installed_at_ms").unwrap_or(0) as u64,
                            source: v.str_field("source").unwrap_or_default().to_string(),
                        });
                    }
                }
            }
        }
        out.sort_by(|a, b| a.package.cmp(&b.package));
        out
    }

    pub fn uninstall(&self, package: &str) -> Result<bool, PmError> {
        let meta = self.dir.join(format!("{package}.json"));
        let dir = self.dir.join(package);
        let existed = meta.exists();
        if meta.exists() {
            std::fs::remove_file(&meta).map_err(PmError::Io)?;
        }
        if dir.exists() {
            std::fs::remove_dir_all(&dir).map_err(PmError::Io)?;
        }
        Ok(existed)
    }
}

/// Load all dexes of a package into the machine's registry.
pub fn load_into(machine: &mut Machine, pkg: &InstalledPackage) -> Result<usize, PmError> {
    let mut count = 0;
    for path in &pkg.dex_paths {
        let bytes = std::fs::read(path).map_err(PmError::Io)?;
        let dex = litebox_dex::Dex::parse(bytes).map_err(PmError::Dex)?;
        machine.registry.add_dex(std::sync::Arc::new(dex));
        count += 1;
    }
    Ok(count)
}

/// A running app instance: the task driving its lifecycle plus the activity
/// object. The owner (VM service) pumps lifecycle phases between scheduler
/// turns so every phase runs at top level and debugger pauses apply.
#[derive(Debug, Clone)]
pub struct AppHandle {
    pub task: u32,
    /// the activity instance as a heap Value
    pub activity: litebox_dalvik::Value,
    pub package: String,
    pub class_desc: String,
    /// 0 = onCreate pending, 1 = onResume pending, 2 = running
    pub phase: u8,
}

impl AppHandle {
    /// Push the next lifecycle phase if the task finished the previous one.
    /// Returns true when a phase was started.
    pub fn pump(&self, machine: &mut Machine) -> Result<bool, PmError> {
        if self.phase > 1 {
            return Ok(false);
        }
        let done = machine
            .tasks
            .task(self.task)
            .map(|t| t.stack.is_empty() || t.state == TaskState::Done)
            .unwrap_or(true);
        if !done {
            return Ok(false);
        }
        let (method, shorty) = match self.phase {
            0 => ("onCreate", "(Landroid/os/Bundle;)V"),
            _ => ("onResume", "()V"),
        };
        let args = match self.phase {
            0 => vec![litebox_dalvik::Value::null()],
            _ => vec![],
        };
        let nid = machine.registry.name_id(method);
        let sid = machine
            .registry
            .intern_shorty(&litebox_dalvik::interp::sig_to_shorty(shorty));
        let cid = machine
            .registry
            .class_by_name(&self.class_desc)
            .ok_or_else(|| PmError::Manifest(format!("activity class {} not loaded", self.class_desc)))?;
        let cls = &machine.registry.classes[cid as usize];
        let gidx = cls
            .vmap
            .get(&(nid.unwrap_or(u32::MAX), sid))
            .or_else(|| cls.dmap.get(&(nid.unwrap_or(u32::MAX), 1)))
            .copied()
            .ok_or_else(|| PmError::Manifest(format!("{} lacks {}{}", self.class_desc, method, shorty)))?;
        let this = self.activity;
        machine
            .invoke_global(self.task, gidx, this, args)
            .map_err(|e| PmError::Manifest(format!("{} push failed: {}", method, e)))?;
        Ok(true)
    }
}

/// Start the package's launcher activity (or a named entry class): create the
/// instance, spawn the main task, and return a handle. Lifecycle phases run
/// via `AppHandle::pump` at top level.
pub fn start_activity(
    machine: &mut Machine,
    pkg: &InstalledPackage,
    activity_override: Option<&str>,
) -> Result<AppHandle, PmError> {
    let activity = activity_override
        .map(String::from)
        .or_else(|| pkg.launcher_activity.clone())
        .ok_or_else(|| PmError::Manifest("no activity declared".into()))?;
    machine.log(
        EventLevel::Info,
        "pm",
        &format!("start {} {}", pkg.package, activity),
    );
    let class_desc = format!("L{};", activity.replace('.', "/"));
    let tid = machine.registry.intern_type(&class_desc);
    machine
        .registry
        .ensure_class(tid)
        .map_err(|e| PmError::Manifest(format!("entry class {class_desc}: {e}")))?;
    machine.init_class(
        machine
            .registry
            .class_by_name(&class_desc)
            .ok_or_else(|| PmError::Manifest("class vanished".into()))?,
    )
    .map_err(|e| PmError::Manifest(format!("clinit: {e}")))?;
    let obj = machine
        .alloc_instance_named(&class_desc)
        .map_err(|spec| PmError::Manifest(format!("alloc: {} {}", spec.class, spec.msg)))?;
    let pkg_name = machine.make_str(&pkg.package);
    machine.set_instance_field(obj, "packageName", pkg_name);
    let task = machine.tasks.spawn("main");
    machine.tasks.current_hint = task;
    Ok(AppHandle {
        task,
        activity: litebox_dalvik::Value::R(obj),
        package: pkg.package.clone(),
        class_desc,
        phase: 0,
    })
}
