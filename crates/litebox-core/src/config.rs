//! Runtime configuration, loaded from CLI flags and/or a JSON config file.

use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PauseOn {
    Off,
    Exception,
    /// exception + first call into an unimplemented framework API
    ExceptionAndUnsupported,
    Everything,
}

#[derive(Debug, Clone)]
pub struct Config {
    /// Directory for installed packages, snapshots, logs. Default ~/.litebox
    pub home: PathBuf,
    /// Framebuffer dimensions for the emulated display surface.
    pub surface_width: u32,
    pub surface_height: u32,
    /// Deterministic seed for app-visible randomness.
    pub seed: u64,
    /// Deterministic starting wall-clock, milliseconds since 1970.
    pub wall_clock_start_ms: u64,
    /// Interpreter instruction slice per scheduler turn.
    pub slice_instructions: u64,
    /// Heap budget in bytes before the compacting GC kicks in.
    pub heap_budget_bytes: u64,
    pub pause_on: PauseOn,
    /// Event ring capacity.
    pub event_capacity: usize,
    /// When set, inputs and app-visible randomness are recorded here.
    pub record_file: Option<PathBuf>,
    /// HTTP listener for the JSON-RPC control plane.
    pub http_addr: Option<String>,
    /// Max instructions for headless `run` before auto-pause (0 = unlimited).
    pub max_instructions: u64,
}

impl Default for Config {
    fn default() -> Self {
        let home = std::env::var_os("LITEBOX_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                let h = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
                h.join(".litebox")
            });
        Config {
            home,
            surface_width: 720,
            surface_height: 1280,
            seed: 0x5EED_0000_0000_0001,
            wall_clock_start_ms: 1_760_000_000_000, // fixed, deterministic default
            slice_instructions: 20_000,
            heap_budget_bytes: 256 * 1024 * 1024,
            pause_on: PauseOn::Exception,
            event_capacity: 65_536,
            record_file: None,
            http_addr: Some("127.0.0.1:7830".to_string()),
            max_instructions: 0,
        }
    }
}

impl Config {
    pub fn packages_dir(&self) -> PathBuf {
        self.home.join("packages")
    }
    pub fn snapshots_dir(&self) -> PathBuf {
        self.home.join("snapshots")
    }
    pub fn data_dir(&self) -> PathBuf {
        self.home.join("data")
    }
    pub fn ensure_dirs(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(self.packages_dir())?;
        std::fs::create_dir_all(self.snapshots_dir())?;
        Ok(())
    }
}
