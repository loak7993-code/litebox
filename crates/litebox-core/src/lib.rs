//! LiteBox kernel: virtual clock, deterministic RNG, bounded event bus,
//! per-app sandbox filesystem, and runtime config. Everything the VM and the
//! framework share lives here, with zero dependencies.

pub mod bus;
pub mod clock;
pub mod config;
pub mod fs;
pub mod rng;
pub mod screen;

pub use bus::{EventBus, EventLevel, EventRecord};
pub use clock::VmClock;
pub use config::Config;
pub use fs::SandboxFs;
pub use rng::Rng;
pub use screen::Screen;

pub type BoxError = Box<dyn std::error::Error + Send + Sync>;
