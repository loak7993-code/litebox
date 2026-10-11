//! VM fault types. Two channels: Java-level exceptions (catchable by dex
//! handlers) and unrecoverable VM faults (internal bugs or resource limits).

use crate::heap::ExFrame;

#[derive(Debug, Clone)]
pub struct ThrowSpec {
    /// class descriptor, e.g. "Ljava/lang/NullPointerException;"
    pub class: String,
    pub msg: String,
    /// extra structured payload for the event stream
    pub data: Vec<(String, String)>,
}

impl ThrowSpec {
    pub fn new(class: &str, msg: impl Into<String>) -> Self {
        ThrowSpec { class: class.to_string(), msg: msg.into(), data: Vec::new() }
    }
    pub fn npe(msg: impl Into<String>) -> Self {
        Self::new("Ljava/lang/NullPointerException;", msg)
    }
    pub fn iae(msg: impl Into<String>) -> Self {
        Self::new("Ljava/lang/IllegalArgumentException;", msg)
    }
    pub fn ioob(msg: impl Into<String>) -> Self {
        Self::new("Ljava/lang/ArrayIndexOutOfBoundsException;", msg)
    }
    pub fn unsupported(msg: impl Into<String>) -> Self {
        Self::new("Llitebox/UnsupportedApiException;", msg)
    }
}

/// A thrown Java exception carrying its object reference and captured frames.
#[derive(Debug, Clone)]
pub struct JavaExn {
    pub obj: crate::value::ObjRef,
    pub class_desc: String,
    pub frames: Vec<ExFrame>,
}

#[derive(Debug)]
pub enum VmFault {
    Java(JavaExn),
    /// unrecoverable: stops the task, reported as a VM error event
    Vm(String),
}

impl std::fmt::Display for VmFault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VmFault::Java(e) => write!(f, "java exception {}: {}", e.class_desc, e.frames.len()),
            VmFault::Vm(m) => write!(f, "vm fault: {m}"),
        }
    }
}
impl std::error::Error for VmFault {}

impl From<String> for VmFault {
    fn from(s: String) -> Self {
        VmFault::Vm(s)
    }
}
impl From<&str> for VmFault {
    fn from(s: &str) -> Self {
        VmFault::Vm(s.to_string())
    }
}
