//! Native method registry: framework implementations callable from bytecode.
//! Instance natives receive `this` as the second argument; static natives
//! receive a NULL object. Registration happens at framework boot, before any
//! dex class is loaded.

use crate::errors::ThrowSpec;
use crate::env::Env;
use std::collections::HashMap;
use std::sync::Arc;

pub type NativeResult = Result<crate::value::Value, ThrowSpec>;
pub type NativeFn =
    Arc<dyn Fn(&mut Env, crate::value::Value, &[crate::value::Value]) -> NativeResult + Send + Sync>;

#[derive(Clone)]
struct NativeEntry {
    f: NativeFn,
    is_static: bool,
}

#[derive(Default, Clone)]
pub struct NativeRegistry {
    map: HashMap<(String, String, String), NativeEntry>,
    /// per-class catch-all for methods not individually registered
    catch_all: HashMap<String, NativeFn>,
}

impl NativeRegistry {
    pub fn new() -> Self {
        NativeRegistry::default()
    }

    pub fn register(
        &mut self,
        class_desc: &str,
        method: &str,
        shorty: &str,
        is_static: bool,
        f: NativeFn,
    ) {
        // normalize: registrations may use full signatures "(IJ)V"; keys are
        // always dex shorty form ("VIJ") so dex-side lookups match.
        let key_shorty = if shorty.contains('(') {
            crate::interp::sig_to_shorty(shorty)
        } else {
            shorty.to_string()
        };
        self.map
            .entry((class_desc.to_string(), method.to_string(), key_shorty))
            .or_insert(NativeEntry { f, is_static });
    }

    pub fn catch_all_for(&mut self, class_desc: &str, f: NativeFn) {
        self.catch_all.insert(class_desc.to_string(), f);
    }

    pub fn lookup(
        &self,
        class_desc: &str,
        method: &str,
        shorty: &str,
    ) -> Option<(&NativeFn, bool)> {
        let key_shorty = if shorty.contains('(') {
            crate::interp::sig_to_shorty(shorty)
        } else {
            shorty.to_string()
        };
        self.map
            .get(&(class_desc.to_string(), method.to_string(), key_shorty))
            .map(|e| (&e.f, e.is_static))
    }

    pub fn lookup_catch_all(&self, class_desc: &str) -> Option<&NativeFn> {
        self.catch_all.get(class_desc)
    }

    /// All natives registered for a class descriptor:
    /// (name, shorty, is_static, function).
    pub fn for_class(&self, class_desc: &str) -> Vec<(String, String, bool, NativeFn)> {
        let mut out = Vec::new();
        for ((c, m, s), e) in &self.map {
            if c == class_desc {
                out.push((m.clone(), s.clone(), e.is_static, e.f.clone()));
            }
        }
        out.sort_by(|a, b| (a.0.as_str(), a.1.as_str()).cmp(&(b.0.as_str(), b.1.as_str())));
        out
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Debug: all (class, method, shorty) keys.
    pub fn keys(&self) -> Vec<(String, String, String)> {
        self.map.keys().cloned().collect()
    }
}
