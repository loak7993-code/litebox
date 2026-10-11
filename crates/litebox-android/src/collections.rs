//! Collections natives: ArrayList (+iterator), HashMap, HashSet.

use litebox_dalvik::env::Env;
use litebox_dalvik::errors::ThrowSpec;
use litebox_dalvik::heap::{ArrayKind, ObjectData};
use litebox_dalvik::value::Value;
use litebox_dalvik::NativeFn;
use std::sync::Arc;

fn f<F>(f: F) -> NativeFn
where
    F: Fn(&mut Env, Value, &[Value]) -> Result<Value, ThrowSpec> + Send + Sync + 'static,
{
    Arc::new(f)
}

/// Read an ArrayList's backing array + size.
pub fn al_parts(e: &mut Env, this: Value) -> (Vec<Value>, i32) {
    let data = e
        .get_field(this, "elementData")
        .and_then(|v| {
            if v.is_null() {
                None
            } else {
                match e.m.heap.get(v.as_ref()) {
                    Some(ObjectData::Array { data, .. }) => Some(data.to_vec()),
                    _ => None,
                }
            }
        })
        .unwrap_or_default();
    let size = e.field_i32(this, "size").unwrap_or(0);
    (data, size)
}

pub fn al_store(e: &mut Env, this: Value, data: Vec<Value>, size: i32) {
    let arr = e.m.alloc_array(ArrayKind::Refs, data.len().max(1));
    if let Some(ObjectData::Array { data: d, .. }) = e.m.heap.get_mut(arr) {
        *d = data.into_boxed_slice();
    }
    e.set_field(this, "elementData", Value::R(arr));
    e.set_field(this, "size", Value::I(size));
}

pub fn register(registry: &mut litebox_dalvik::NativeRegistry) {
    const AL: &str = "Ljava/util/ArrayList;";
    registry.register(AL, "<init>", "()V", false, f(|e, this, _a| {
        al_store(e, this, Vec::new(), 0);
        Ok(Value::null())
    }));
    registry.register(AL, "<init>", "(I)V", false, f(|e, this, _a| {
        al_store(e, this, Vec::new(), 0);
        Ok(Value::null())
    }));
    registry.register(AL, "size", "()I", false, f(|e, this, _a| {
        Ok(Value::I(al_parts(e, this).1))
    }));
    registry.register(AL, "isEmpty", "()Z", false, f(|e, this, _a| {
        Ok(Value::I((al_parts(e, this).1 == 0) as i32))
    }));
    registry.register(AL, "clear", "()V", false, f(|e, this, _a| {
        al_store(e, this, Vec::new(), 0);
        Ok(Value::null())
    }));
    registry.register(AL, "add", "(Ljava/lang/Object;)Z", false, f(|e, this, a| {
        let (mut data, size) = al_parts(e, this);
        data.resize(size as usize, Value::null());
        data.push(e.arg_ref(a, 0));
        al_store(e, this, data, size + 1);
        Ok(Value::I(1))
    }));
    registry.register(AL, "add", "(ILjava/lang/Object;)V", false, f(|e, this, a| {
        let (mut data, size) = al_parts(e, this);
        let idx = e.arg_i(a, 0).max(0) as usize;
        if idx > size as usize {
            return Err(e.throw("Ljava/lang/IndexOutOfBoundsException;", format!("index {idx}, size {size}")));
        }
        data.resize(size as usize, Value::null());
        data.insert(idx, e.arg_ref(a, 1));
        al_store(e, this, data, size + 1);
        Ok(Value::null())
    }));
    registry.register(AL, "get", "(I)Ljava/lang/Object;", false, f(|e, this, a| {
        let (data, size) = al_parts(e, this);
        let idx = e.arg_i(a, 0);
        if idx < 0 || idx >= size {
            return Err(e.throw("Ljava/lang/IndexOutOfBoundsException;", format!("index {idx}, size {size}")));
        }
        Ok(data[idx as usize])
    }));
    registry.register(AL, "set", "(ILjava/lang/Object;)Ljava/lang/Object;", false, f(|e, this, a| {
        let (mut data, size) = al_parts(e, this);
        let idx = e.arg_i(a, 0);
        if idx < 0 || idx >= size {
            return Err(e.throw("Ljava/lang/IndexOutOfBoundsException;", format!("index {idx}, size {size}")));
        }
        let old = data[idx as usize];
        data[idx as usize] = e.arg_ref(a, 1);
        al_store(e, this, data, size);
        Ok(old)
    }));
    registry.register(AL, "remove", "(I)Ljava/lang/Object;", false, f(|e, this, a| {
        let (mut data, size) = al_parts(e, this);
        let idx = e.arg_i(a, 0);
        if idx < 0 || idx >= size {
            return Err(e.throw("Ljava/lang/IndexOutOfBoundsException;", format!("index {idx}, size {size}")));
        }
        let old = data.remove(idx as usize);
        al_store(e, this, data, size - 1);
        Ok(old)
    }));
    registry.register(AL, "remove", "(Ljava/lang/Object;)Z", false, f(|e, this, a| {
        let (mut data, size) = al_parts(e, this);
        let target = e.arg_ref(a, 0);
        let pos = data.iter().position(|v| value_eq(e, *v, target));
        match pos {
            Some(p) => {
                data.remove(p);
                al_store(e, this, data, size - 1);
                Ok(Value::I(1))
            }
            None => Ok(Value::I(0)),
        }
    }));
    registry.register(AL, "contains", "(Ljava/lang/Object;)Z", false, f(|e, this, a| {
        let (data, _) = al_parts(e, this);
        let target = e.arg_ref(a, 0);
        Ok(Value::I(data.iter().any(|v| value_eq(e, *v, target)) as i32))
    }));
    registry.register(AL, "indexOf", "(Ljava/lang/Object;)I", false, f(|e, this, a| {
        let (data, _) = al_parts(e, this);
        let target = e.arg_ref(a, 0);
        Ok(Value::I(
            data.iter().position(|v| value_eq(e, *v, target)).map(|p| p as i32).unwrap_or(-1),
        ))
    }));
    registry.register(AL, "iterator", "()Ljava/util/Iterator;", false, f(|e, this, _a| {
        let itr = e.alloc("Ljava/util/ArrayList$Itr;")?;
        e.set_field(itr, "list", this);
        e.set_field(itr, "cursor", Value::I(0));
        Ok(itr)
    }));
    registry.register(AL, "toString", "()Ljava/lang/String;", false, f(|e, this, _a| {
        let (data, size) = al_parts(e, this);
        let mut parts = Vec::new();
        for v in data.into_iter().take(size as usize) {
            if v.is_null() {
                parts.push("null".to_string());
            } else {
                let sv = e.call(v, "Ljava/lang/Object;", "toString", "()Ljava/lang/String;", vec![])?;
                let s = e.m.str_value(sv).unwrap_or_else(|| "null".to_string());
                parts.push(s);
            }
        }
        Ok(e.make_str(&format!("[{}]", parts.join(", "))))
    }));

    // iterator
    const IT: &str = "Ljava/util/ArrayList$Itr;";
    registry.register(IT, "hasNext", "()Z", false, f(|e, this, _a| {
        let list = e.get_field(this, "list").unwrap_or(Value::null());
        let cur = e.field_i32(this, "cursor").unwrap_or(0);
        Ok(Value::I((cur < al_parts(e, list).1) as i32))
    }));
    registry.register(IT, "next", "()Ljava/lang/Object;", false, f(|e, this, _a| {
        let list = e.get_field(this, "list").unwrap_or(Value::null());
        let (data, size) = al_parts(e, list);
        let cur = e.field_i32(this, "cursor").unwrap_or(0);
        if cur >= size {
            return Err(e.throw("Ljava/util/NoSuchElementException;", "iterator exhausted"));
        }
        e.set_field(this, "cursor", Value::I(cur + 1));
        Ok(data[cur as usize])
    }));
    // HashMap: linear-scan map with insertion-ordered iteration
    const HM: &str = "Ljava/util/HashMap;";
    registry.register(HM, "<init>", "()V", false, f(|e, this, _a| {
        let karr = e.m.alloc_array(ArrayKind::Refs, 0);
        let varr = e.m.alloc_array(ArrayKind::Refs, 0);
        e.set_field(this, "keys", Value::R(karr));
        e.set_field(this, "vals", Value::R(varr));
        e.set_field(this, "size", Value::I(0));
        Ok(Value::null())
    }));
    registry.register(HM, "size", "()I", false, f(|e, this, _a| {
        Ok(Value::I(e.field_i32(this, "size").unwrap_or(0)))
    }));
    registry.register(HM, "isEmpty", "()Z", false, f(|e, this, _a| {
        Ok(Value::I((e.field_i32(this, "size").unwrap_or(0) == 0) as i32))
    }));
    registry.register(HM, "put", "(Ljava/lang/Object;Ljava/lang/Object;)Ljava/lang/Object;", false, f(|e, this, a| {
        let (keys, vals) = hm_parts(e, this);
        let k = e.arg_ref(a, 0);
        if k.is_null() {
            return Err(e.throw_npe("null keys unsupported in lite HashMap"));
        }
        let v = e.arg_ref(a, 1);
        for (i, ek) in keys.iter().enumerate() {
            if value_eq(e, *ek, k) {
                let old = vals[i];
                let mut nv = vals.clone();
                nv[i] = v;
                let nkeys = keys.len() as i32;
                hm_store(e, this, keys, nv, nkeys);
                return Ok(old);
            }
        }
        let mut nk = keys;
        nk.push(k);
        let mut nv = vals;
        nv.push(v);
        let n = e.field_i32(this, "size").unwrap_or(0);
        hm_store(e, this, nk, nv, n + 1);
        Ok(Value::null())
    }));
    registry.register(HM, "get", "(Ljava/lang/Object;)Ljava/lang/Object;", false, f(|e, this, a| {
        let (keys, vals) = hm_parts(e, this);
        let k = e.arg_ref(a, 0);
        for (i, ek) in keys.iter().enumerate() {
            if value_eq(e, *ek, k) {
                return Ok(vals[i]);
            }
        }
        Ok(Value::null())
    }));
    registry.register(HM, "getOrDefault", "(Ljava/lang/Object;Ljava/lang/Object;)Ljava/lang/Object;", false, f(|e, this, a| {
        let (keys, vals) = hm_parts(e, this);
        let k = e.arg_ref(a, 0);
        for (i, ek) in keys.iter().enumerate() {
            if value_eq(e, *ek, k) {
                return Ok(vals[i]);
            }
        }
        Ok(e.arg_ref(a, 1))
    }));
    registry.register(HM, "containsKey", "(Ljava/lang/Object;)Z", false, f(|e, this, a| {
        let (keys, _) = hm_parts(e, this);
        let k = e.arg_ref(a, 0);
        Ok(Value::I(keys.iter().any(|ek| value_eq(e, *ek, k)) as i32))
    }));
    registry.register(HM, "remove", "(Ljava/lang/Object;)Ljava/lang/Object;", false, f(|e, this, a| {
        let (keys, vals) = hm_parts(e, this);
        let k = e.arg_ref(a, 0);
        for i in 0..keys.len() {
            if value_eq(e, keys[i], k) {
                let old = vals[i];
                let mut nk = keys.clone();
                nk.remove(i);
                let mut nv = vals.clone();
                nv.remove(i);
                let n = e.field_i32(this, "size").unwrap_or(0);
                hm_store(e, this, nk, nv, (n - 1).max(0));
                return Ok(old);
            }
        }
        Ok(Value::null())
    }));
    registry.register(HM, "keySet", "()Ljava/util/Set;", false, f(|e, this, _a| {
        let (keys, _) = hm_parts(e, this);
        let size = keys.len() as i32;
        let set = e.alloc("Ljava/util/HashSet;")?;
        let map = e.alloc("Ljava/util/HashMap;")?;
        let nv = vec![Value::null(); keys.len()];
        hm_store(e, map, keys, nv, size);
        e.set_field(set, "map", map);
        Ok(set)
    }));
    registry.register(HM, "values", "()Ljava/util/Collection;", false, f(|e, this, _a| {
        let (_, vals) = hm_parts(e, this);
        let len = vals.len() as i32;
        let list = e.alloc("Ljava/util/ArrayList;")?;
        al_store(e, list, vals, len);
        Ok(list)
    }));
    registry.register(HM, "clear", "()V", false, f(|e, this, _a| {
        hm_store(e, this, Vec::new(), Vec::new(), 0);
        Ok(Value::null())
    }));
    registry.register(HM, "toString", "()Ljava/lang/String;", false, f(|e, this, _a| {
        let (keys, vals) = hm_parts(e, this);
        let mut parts = Vec::new();
        for (k, v) in keys.iter().zip(vals.iter()) {
            let ks = if k.is_null() { "null".into() } else {
                let kv = e.call(*k, "Ljava/lang/Object;", "toString", "()Ljava/lang/String;", vec![])?;
                e.m.str_value(kv).unwrap_or_else(|| "null".to_string())
            };
            let vs = if v.is_null() { "null".into() } else {
                let sv = e.call(*v, "Ljava/lang/Object;", "toString", "()Ljava/lang/String;", vec![])?;
                e.m.str_value(sv).unwrap_or_else(|| "null".to_string())
            };
            parts.push(format!("{ks}={vs}"));
        }
        Ok(e.make_str(&format!("{{{}}}", parts.join(", "))))
    }));

    // HashSet over the map field
    const HS: &str = "Ljava/util/HashSet;";
    registry.register(HS, "<init>", "()V", false, f(|e, this, _a| {
        let map = e.alloc("Ljava/util/HashMap;")?;
        e.set_field(this, "map", map);
        Ok(Value::null())
    }));
    registry.register(HS, "add", "(Ljava/lang/Object;)Z", false, f(|e, this, a| {
        let map = e.get_field(this, "map").unwrap_or(Value::null());
        let marker = e.make_str("1");
        let r = e.call(map, "Ljava/util/HashMap;", "put", "(Ljava/lang/Object;Ljava/lang/Object;)Ljava/lang/Object;", vec![
            e.arg_ref(a, 0),
            marker,
        ])?;
        Ok(Value::I(r.is_null() as i32))
    }));
    registry.register(HS, "contains", "(Ljava/lang/Object;)Z", false, f(|e, this, a| {
        let map = e.get_field(this, "map").unwrap_or(Value::null());
        e.call(map, "Ljava/util/HashMap;", "containsKey", "(Ljava/lang/Object;)Z", vec![e.arg_ref(a, 0)])
    }));
    registry.register(HS, "size", "()I", false, f(|e, this, _a| {
        let map = e.get_field(this, "map").unwrap_or(Value::null());
        e.call(map, "Ljava/util/HashMap;", "size", "()I", vec![])
    }));
    registry.register(HS, "isEmpty", "()Z", false, f(|e, this, _a| {
        let map = e.get_field(this, "map").unwrap_or(Value::null());
        e.call(map, "Ljava/util/HashMap;", "isEmpty", "()Z", vec![])
    }));
    registry.register(HS, "remove", "(Ljava/lang/Object;)Z", false, f(|e, this, a| {
        let map = e.get_field(this, "map").unwrap_or(Value::null());
        let old = e.call(map, "Ljava/util/HashMap;", "remove", "(Ljava/lang/Object;)Ljava/lang/Object;", vec![e.arg_ref(a, 0)])?;
        Ok(Value::I(!old.is_null() as i32))
    }));
    registry.register(HS, "clear", "()V", false, f(|e, this, _a| {
        let map = e.get_field(this, "map").unwrap_or(Value::null());
        e.call(map, "Ljava/util/HashMap;", "clear", "()V", vec![])
    }));
}

fn hm_parts(e: &mut Env, this: Value) -> (Vec<Value>, Vec<Value>) {
    let read = |fld: &str| -> Vec<Value> {
        e.get_field(this, fld)
            .and_then(|v| {
                if v.is_null() {
                    None
                } else {
                    match e.m.heap.get(v.as_ref()) {
                        Some(ObjectData::Array { data, .. }) => Some(data.to_vec()),
                        _ => None,
                    }
                }
            })
            .unwrap_or_default()
    };
    (read("keys"), read("vals"))
}

fn hm_store(e: &mut Env, this: Value, keys: Vec<Value>, vals: Vec<Value>, size: i32) {
    for (fld, data) in [("keys", keys), ("vals", vals)] {
        let arr = e.m.alloc_array(ArrayKind::Refs, data.len().max(1));
        if let Some(ObjectData::Array { data: d, .. }) = e.m.heap.get_mut(arr) {
            *d = data.into_boxed_slice();
        }
        e.set_field(this, fld, Value::R(arr));
    }
    e.set_field(this, "size", Value::I(size));
}

pub fn value_eq(e: &mut Env, a: Value, b: Value) -> bool {
    if a == b {
        return true;
    }
    if a.is_null() || b.is_null() {
        return false;
    }
    // Strings compare by content when both are the same class
    if let (Some(x), Some(y)) = (e.m.str_of(a.as_ref()), e.m.str_of(b.as_ref())) {
        let ac = e.class_of(a);
        let bc = e.class_of(b);
        if ac.is_some() && ac == bc {
            let scls = e.m.registry.class_by_name("Ljava/lang/String;");
            if ac == scls {
                return x == y;
            }
        }
    }
    // boxed numbers compare by value
    let av = e.get_field(a, "value");
    let bv = e.get_field(b, "value");
    if let (Some(x), Some(y)) = (av, bv) {
        if x == y {
            return true;
        }
        if matches!(x, Value::I(_) | Value::L(_)) && matches!(y, Value::I(_) | Value::L(_)) {
            return x.as_i64() == y.as_i64();
        }
    }
    false
}


