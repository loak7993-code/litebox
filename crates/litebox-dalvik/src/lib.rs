//! The Dalvik-class virtual machine: DEX execution engine with object model,
//! cooperative tasks, native bindings, and debugger state.

pub mod classes;
pub mod debug;
pub mod decode;
pub mod decode_types;
pub mod env;
pub mod errors;
pub mod heap;
pub mod interp;
pub mod natives;
pub mod snapshot;
pub mod tasks;
pub mod value;

pub use classes::{Registry, RuntimeClass};
pub use debug::{DebugState, PauseReason};
pub use env::Env;
pub use errors::{JavaExn, ThrowSpec, VmFault};
pub use heap::{ArrayKind, Heap, ObjectData};
pub use interp::{Machine, RunOutcome, VmStats};
pub use natives::{NativeFn, NativeRegistry};
pub use snapshot::{load, save};
pub use tasks::{Frame, Task, TaskSet, TaskState};
pub use value::{ObjRef, Value};
