//! Environment handed to native method implementations. Thin ergonomic
//! wrappers over Machine: string ops, field access, nested calls, throwing,
//! logging, and direct access to the kernel devices.

use crate::errors::ThrowSpec;
use crate::interp::Machine;
use crate::value::Value;
use litebox_core::EventLevel;

pub struct Env<'m> {
    pub m: &'m mut Machine,
}

impl<'m> Env<'m> {
    // ---- strings ----
    pub fn str_of(&self, v: Value) -> Option<String> {
        self.m.str_value(v)
    }
    pub fn make_str(&mut self, s: &str) -> Value {
        self.m.make_str(s)
    }

    // ---- objects ----
    pub fn alloc(&mut self, class_desc: &str) -> Result<Value, ThrowSpec> {
        self.m.alloc_instance_named(class_desc).map(Value::R)
    }
    pub fn class_of(&self, v: Value) -> Option<u32> {
        self.m.object_class_of(v)
    }
    pub fn get_field(&self, obj: Value, name: &str) -> Option<Value> {
        if obj.is_null() {
            return None;
        }
        self.m.get_instance_field(obj.as_ref(), name)
    }
    pub fn set_field(&mut self, obj: Value, name: &str, v: Value) -> bool {
        if obj.is_null() {
            return false;
        }
        self.m.set_instance_field(obj.as_ref(), name, v)
    }
    pub fn field_str(&self, obj: Value, name: &str) -> Option<String> {
        self.get_field(obj, name).and_then(|v| self.m.str_value(v))
    }
    pub fn field_i32(&self, obj: Value, name: &str) -> Option<i32> {
        self.get_field(obj, name).map(|v| v.as_i32())
    }
    pub fn field_i64(&self, obj: Value, name: &str) -> Option<i64> {
        self.get_field(obj, name).map(|v| v.as_i64())
    }

    // ---- calls ----
    /// Call a method and run it to completion. `this` = NULL for statics.
    pub fn call(
        &mut self,
        this: Value,
        class_desc: &str,
        method: &str,
        shorty: &str,
        args: Vec<Value>,
    ) -> Result<Value, ThrowSpec> {
        match self.m.call_method_nested(this, class_desc, method, shorty, args) {
            Ok(v) => Ok(v),
            Err(exn) => Err(ThrowSpec::new(
                &exn.class_desc,
                self.m.exn_message(&exn),
            )),
        }
    }

    /// Call a static method with typed args.
    pub fn call_static(
        &mut self,
        class_desc: &str,
        method: &str,
        shorty: &str,
        args: Vec<Value>,
    ) -> Result<Value, ThrowSpec> {
        self.call(Value::null(), class_desc, method, shorty, args)
    }

    // ---- throwing ----
    pub fn throw(&self, class_desc: &str, msg: impl Into<String>) -> ThrowSpec {
        ThrowSpec::new(class_desc, msg)
    }
    pub fn throw_unsupported(&self, msg: impl Into<String>) -> ThrowSpec {
        ThrowSpec::unsupported(msg)
    }
    pub fn throw_npe(&self, msg: impl Into<String>) -> ThrowSpec {
        ThrowSpec::npe(msg)
    }

    // ---- logging ----
    pub fn log(&mut self, level: EventLevel, tag: &str, msg: &str) {
        self.m.log(level, tag, msg);
    }
    pub fn log_d(&mut self, tag: &str, msg: &str) {
        self.m.log(EventLevel::Debug, tag, msg);
    }
    pub fn log_i(&mut self, tag: &str, msg: &str) {
        self.m.log(EventLevel::Info, tag, msg);
    }
    pub fn log_w(&mut self, tag: &str, msg: &str) {
        self.m.log(EventLevel::Warn, tag, msg);
    }
    pub fn log_e(&mut self, tag: &str, msg: &str) {
        self.m.log(EventLevel::Error, tag, msg);
    }

    // ---- arg readers (natives convenience) ----
    pub fn arg_i(&self, args: &[Value], i: usize) -> i32 {
        args.get(i).map(|v| v.as_i32()).unwrap_or(0)
    }
    pub fn arg_l(&self, args: &[Value], i: usize) -> i64 {
        args.get(i).map(|v| v.as_i64()).unwrap_or(0)
    }
    pub fn arg_f(&self, args: &[Value], i: usize) -> f32 {
        args.get(i).map(|v| v.as_f32()).unwrap_or(0.0)
    }
    pub fn arg_d(&self, args: &[Value], i: usize) -> f64 {
        args.get(i).map(|v| v.as_f64()).unwrap_or(0.0)
    }
    pub fn arg_ref(&self, args: &[Value], i: usize) -> Value {
        args.get(i).copied().unwrap_or(Value::null())
    }
    pub fn arg_str(&self, args: &[Value], i: usize) -> Option<String> {
        self.m.str_value(self.arg_ref(args, i))
    }

    // ---- devices ----
    pub fn now_millis(&self) -> u64 {
        self.m.wall_clock_start_ms + self.m.clock.millis()
    }
    pub fn uptime_millis(&self) -> u64 {
        self.m.clock.millis()
    }
    pub fn nano_time(&self) -> i64 {
        (self.m.clock.now_us() * 1_000) as i64
    }
    pub fn random_i32(&mut self) -> i32 {
        self.m.rng.next_i32()
    }
    pub fn fs(&mut self) -> &mut litebox_core::SandboxFs {
        &mut self.m.fs
    }
    pub fn screen(&mut self) -> &mut litebox_core::Screen {
        &mut self.m.screen
    }
    pub fn bus(&mut self) -> &mut litebox_core::EventBus {
        &mut self.m.bus
    }
}
