//! Native method implementations for java.* core classes.

use litebox_dalvik::env::Env;
use litebox_dalvik::errors::ThrowSpec;
use litebox_dalvik::heap::ArrayKind;
use litebox_dalvik::value::Value;
use litebox_dalvik::NativeFn;
use std::sync::Arc;

fn f<F>(f: F) -> NativeFn
where
    F: Fn(&mut Env, Value, &[Value]) -> Result<Value, ThrowSpec> + Send + Sync + 'static,
{
    Arc::new(f)
}

pub fn register_java(registry: &mut litebox_dalvik::NativeRegistry) {
    // ---------- java/lang/Object ----------
    registry.register("Ljava/lang/Object;", "hashCode", "()I", true, f(|_e, this, _a| {
        Ok(Value::I(match this {
            Value::R(r) => r.idx as i32,
            _ => 0,
        }))
    }));
    registry.register("Ljava/lang/Object;", "toString", "()Ljava/lang/String;", false, f(|e, this, _a| {
        let cls = e.class_of(this).map(|c| e.m.registry.class_name(c).to_string()).unwrap_or_else(|| "null".into());
        let id = match this { Value::R(r) => r.idx, _ => 0 };
        Ok(e.make_str(&format!("{cls}@{id:x}")))
    }));
    registry.register("Ljava/lang/Object;", "equals", "(Ljava/lang/Object;)Z", false, f(|_e, this, a| {
        Ok(Value::I(if this == a.first().copied().unwrap_or(Value::null()) { 1 } else { 0 }))
    }));

    // ---------- java/lang/String ----------
    let sreg = |r: &mut litebox_dalvik::NativeRegistry, name: &str, shorty: &str, is_static: bool, f2: NativeFn| {
        r.register("Ljava/lang/String;", name, shorty, is_static, f2);
    };
    sreg(registry, "length", "()I", false, f(|e, this, _a| {
        Ok(Value::I(e.str_of(this).map(|s| s.chars().count() as i32).unwrap_or(0)))
    }));
    sreg(registry, "charAt", "(I)C", false, f(|e, this, a| {
        let s = e.str_of(this).ok_or_else(|| e.throw_npe("null string"))?;
        let i = e.arg_i(a, 0) as usize;
        s.chars().nth(i).map(|c| Value::I(c as i32)).ok_or_else(|| {
            e.throw("Ljava/lang/StringIndexOutOfBoundsException;", format!("index {i}, length {}", s.chars().count()))
        })
    }));
    sreg(registry, "equals", "(Ljava/lang/Object;)Z", false, f(|e, this, a| {
        let other = e.arg_ref(a, 0);
        let same = match (e.str_of(this), e.str_of(other)) {
            (Some(x), Some(y)) => x == y,
            _ => false,
        };
        Ok(Value::I(same as i32))
    }));
    sreg(registry, "equalsIgnoreCase", "(Ljava/lang/String;)Z", false, f(|e, this, a| {
        let other = e.arg_str(a, 0);
        Ok(Value::I(match (e.str_of(this), other) {
            (Some(x), Some(y)) => x.eq_ignore_ascii_case(&y) as i32,
            _ => 0,
        }))
    }));
    sreg(registry, "hashCode", "()I", false, f(|e, this, _a| {
        // Java String.hashCode: s[0]*31^(n-1) + ...
        let h = e.str_of(this).map(|s| {
            let mut h: i32 = 0;
            for c in s.chars() {
                h = h.wrapping_mul(31).wrapping_add(c as i32);
            }
            h
        }).unwrap_or(0);
        Ok(Value::I(h))
    }));
    sreg(registry, "indexOf", "(Ljava/lang/String;)I", false, f(|e, this, a| {
        let hay = e.str_of(this).unwrap_or_default();
        let needle = e.arg_str(a, 0).unwrap_or_default();
        Ok(Value::I(hay.find(&needle).map(|i| hay[..i].chars().count() as i32).unwrap_or(-1)))
    }));
    sreg(registry, "indexOf", "(I)I", false, f(|e, this, a| {
        let hay = e.str_of(this).unwrap_or_default();
        let c = char::from_u32(e.arg_i(a, 0) as u32).unwrap_or('\u{FFFD}');
        Ok(Value::I(hay.find(c).map(|i| hay[..i].chars().count() as i32).unwrap_or(-1)))
    }));
    sreg(registry, "contains", "(Ljava/lang/CharSequence;)Z", false, f(|e, this, a| {
        let hay = e.str_of(this).unwrap_or_default();
        let needle = e.arg_str(a, 0).unwrap_or_default();
        Ok(Value::I(hay.contains(&needle) as i32))
    }));
    sreg(registry, "startsWith", "(Ljava/lang/String;)Z", false, f(|e, this, a| {
        let hay = e.str_of(this).unwrap_or_default();
        Ok(Value::I(hay.starts_with(&e.arg_str(a, 0).unwrap_or_default()) as i32))
    }));
    sreg(registry, "endsWith", "(Ljava/lang/String;)Z", false, f(|e, this, a| {
        let hay = e.str_of(this).unwrap_or_default();
        Ok(Value::I(hay.ends_with(&e.arg_str(a, 0).unwrap_or_default()) as i32))
    }));
    sreg(registry, "isEmpty", "()Z", false, f(|e, this, _a| {
        Ok(Value::I(e.str_of(this).map(|s| s.is_empty() as i32).unwrap_or(1)))
    }));
    sreg(registry, "substring", "(I)Ljava/lang/String;", false, f(|e, this, a| {
        let s = e.str_of(this).ok_or_else(|| e.throw_npe("null string"))?;
        let chars: Vec<char> = s.chars().collect();
        let start = e.arg_i(a, 0) as usize;
        if start > chars.len() {
            return Err(e.throw("Ljava/lang/StringIndexOutOfBoundsException;", format!("begin {start}, length {}", chars.len())));
        }
        Ok(e.make_str(&chars[start..].iter().collect::<String>()))
    }));
    sreg(registry, "substring", "(II)Ljava/lang/String;", false, f(|e, this, a| {
        let s = e.str_of(this).ok_or_else(|| e.throw_npe("null string"))?;
        let chars: Vec<char> = s.chars().collect();
        let start = e.arg_i(a, 0) as usize;
        let end = e.arg_i(a, 1) as usize;
        if start > end || end > chars.len() {
            return Err(e.throw("Ljava/lang/StringIndexOutOfBoundsException;", format!("begin {start}, end {end}, length {}", chars.len())));
        }
        Ok(e.make_str(&chars[start..end].iter().collect::<String>()))
    }));
    sreg(registry, "concat", "(Ljava/lang/String;)Ljava/lang/String;", false, f(|e, this, a| {
        let mut s = e.str_of(this).unwrap_or_default();
        s.push_str(&e.arg_str(a, 0).unwrap_or_default());
        Ok(e.make_str(&s))
    }));
    sreg(registry, "toUpperCase", "()Ljava/lang/String;", false, f(|e, this, _a| {
        Ok(e.make_str(&e.str_of(this).unwrap_or_default().to_uppercase()))
    }));
    sreg(registry, "toLowerCase", "()Ljava/lang/String;", false, f(|e, this, _a| {
        Ok(e.make_str(&e.str_of(this).unwrap_or_default().to_lowercase()))
    }));
    sreg(registry, "trim", "()Ljava/lang/String;", false, f(|e, this, _a| {
        Ok(e.make_str(e.str_of(this).unwrap_or_default().trim_matches(|c: char| c <= ' ')))
    }));
    sreg(registry, "replace", "(CC)Ljava/lang/String;", false, f(|e, this, a| {
        let from = char::from_u32(e.arg_i(a, 0) as u32).unwrap_or('\u{FFFD}');
        let to = char::from_u32(e.arg_i(a, 1) as u32).unwrap_or('\u{FFFD}');
        Ok(e.make_str(&e.str_of(this).unwrap_or_default().replace(from, &to.to_string())))
    }));
    sreg(registry, "replace", "(Ljava/lang/CharSequence;Ljava/lang/CharSequence;)Ljava/lang/String;", false, f(|e, this, a| {
        let from = e.arg_str(a, 0).unwrap_or_default();
        let to = e.arg_str(a, 1).unwrap_or_default();
        Ok(e.make_str(&e.str_of(this).unwrap_or_default().replace(&from, &to)))
    }));
    sreg(registry, "compareTo", "(Ljava/lang/String;)I", false, f(|e, this, a| {
        let x = e.str_of(this).unwrap_or_default();
        let y = e.arg_str(a, 0).unwrap_or_default();
        let xc: Vec<u16> = x.encode_utf16().collect();
        let yc: Vec<u16> = y.encode_utf16().collect();
        let mut r = 0i32;
        for i in 0..xc.len().min(yc.len()) {
            if xc[i] != yc[i] {
                r = xc[i] as i32 - yc[i] as i32;
                break;
            }
        }
        if r == 0 {
            r = xc.len() as i32 - yc.len() as i32;
        }
        Ok(Value::I(r))
    }));
    sreg(registry, "split", "(Ljava/lang/String;)[Ljava/lang/String;", false, f(|e, this, a| {
        let s = e.str_of(this).unwrap_or_default();
        let sep = e.arg_str(a, 0).unwrap_or_default();
        // literal separator split (regex metachars unsupported; documented)
        let parts: Vec<String> = s.split(sep.as_str()).map(|p| p.to_string()).collect();
        Ok(e.m.make_str_array(&parts))
    }));
    sreg(registry, "toString", "()Ljava/lang/String;", false, f(|_e, this, _a| Ok(this)));
    sreg(registry, "intern", "()Ljava/lang/String;", false, f(|_e, this, _a| Ok(this)));
    sreg(registry, "getBytes", "()[B", false, f(|e, this, _a| {
        let s = e.str_of(this).unwrap_or_default();
        Ok(e.m.make_byte_array(s.as_bytes()))
    }));
    sreg(registry, "repeat", "(I)Ljava/lang/String;", false, f(|e, this, a| {
        let s = e.str_of(this).unwrap_or_default();
        let n = e.arg_i(a, 0).max(0) as usize;
        Ok(e.make_str(&s.repeat(n)))
    }));
    sreg(registry, "valueOf", "(I)Ljava/lang/String;", true, f(|e, _t, a| Ok(e.make_str(&e.arg_i(a, 0).to_string()))));
    sreg(registry, "valueOf", "(J)Ljava/lang/String;", true, f(|e, _t, a| Ok(e.make_str(&e.arg_l(a, 0).to_string()))));
    sreg(registry, "valueOf", "(Z)Ljava/lang/String;", true, f(|e, _t, a| Ok(e.make_str(if e.arg_i(a, 0) != 0 { "true" } else { "false" }))));
    sreg(registry, "valueOf", "(C)Ljava/lang/String;", true, f(|e, _t, a| {
        let c = char::from_u32(e.arg_i(a, 0) as u32).unwrap_or('\u{FFFD}');
        Ok(e.make_str(&c.to_string()))
    }));
    sreg(registry, "valueOf", "(F)Ljava/lang/String;", true, f(|e, _t, a| Ok(e.make_str(&java_float_to_string(e.arg_f(a, 0))))));
    sreg(registry, "valueOf", "(D)Ljava/lang/String;", true, f(|e, _t, a| Ok(e.make_str(&java_double_to_string(e.arg_d(a, 0))))));
    sreg(registry, "valueOf", "(Ljava/lang/Object;)Ljava/lang/String;", true, f(|e, _t, a| {
        let v = e.arg_ref(a, 0);
        if v.is_null() {
            return Ok(e.make_str("null"));
        }
        let s = e.call(v, "Ljava/lang/Object;", "toString", "()Ljava/lang/String;", vec![])?;
        Ok(s)
    }));
    // constructors
    sreg(registry, "<init>", "()V", false, f(|_e, _t, _a| Ok(Value::null())));
    sreg(registry, "<init>", "(Ljava/lang/String;)V", false, f(|e, this, a| {
        if let Some(s) = e.arg_str(a, 0) {
            e.m.heap_set_str(this.as_ref(), &s);
        }
        Ok(Value::null())
    }));
    sreg(registry, "<init>", "([B)V", false, f(|e, this, a| {
        let bytes = e.m.byte_array_of(e.arg_ref(a, 0)).unwrap_or_default();
        e.m.heap_set_str(this.as_ref(), &String::from_utf8_lossy(&bytes));
        Ok(Value::null())
    }));

    // ---------- java/lang/StringBuilder ----------
    let sb = "Ljava/lang/StringBuilder;";
    registry.register(sb, "append", "(Ljava/lang/String;)Ljava/lang/StringBuilder;", false, f(|e, this, a| {
        let cur = e.field_str(this, "s").unwrap_or_default();
        let add = e.arg_str(a, 0).unwrap_or_default();
        let merged = format!("{cur}{add}");
        e.m.make_str(&merged);
        let sv = e.make_str(&merged);
        e.set_field(this, "s", sv);
        Ok(this)
    }));
    registry.register(sb, "append", "(I)Ljava/lang/StringBuilder;", false, f(|e, this, a| {
        let add = e.arg_i(a, 0).to_string();
        let merged = format!("{}{add}", e.field_str(this, "s").unwrap_or_default());
        let sv = e.make_str(&merged);
        e.set_field(this, "s", sv);
        Ok(this)
    }));
    registry.register(sb, "append", "(J)Ljava/lang/StringBuilder;", false, f(|e, this, a| {
        let add = e.arg_l(a, 0).to_string();
        let merged = format!("{}{add}", e.field_str(this, "s").unwrap_or_default());
        let sv = e.make_str(&merged);
        e.set_field(this, "s", sv);
        Ok(this)
    }));
    registry.register(sb, "append", "(C)Ljava/lang/StringBuilder;", false, f(|e, this, a| {
        let add = char::from_u32(e.arg_i(a, 0) as u32).unwrap_or('?').to_string();
        let merged = format!("{}{add}", e.field_str(this, "s").unwrap_or_default());
        let sv = e.make_str(&merged);
        e.set_field(this, "s", sv);
        Ok(this)
    }));
    registry.register(sb, "append", "(Z)Ljava/lang/StringBuilder;", false, f(|e, this, a| {
        let add = if e.arg_i(a, 0) != 0 { "true".to_string() } else { "false".to_string() };
        let merged = format!("{}{add}", e.field_str(this, "s").unwrap_or_default());
        let sv = e.make_str(&merged);
        e.set_field(this, "s", sv);
        Ok(this)
    }));
    registry.register(sb, "append", "(F)Ljava/lang/StringBuilder;", false, f(|e, this, a| {
        let add = java_float_to_string(e.arg_f(a, 0));
        let merged = format!("{}{add}", e.field_str(this, "s").unwrap_or_default());
        let sv = e.make_str(&merged);
        e.set_field(this, "s", sv);
        Ok(this)
    }));
    registry.register(sb, "append", "(D)Ljava/lang/StringBuilder;", false, f(|e, this, a| {
        let add = java_double_to_string(e.arg_d(a, 0));
        let merged = format!("{}{add}", e.field_str(this, "s").unwrap_or_default());
        let sv = e.make_str(&merged);
        e.set_field(this, "s", sv);
        Ok(this)
    }));
    registry.register(sb, "append", "(Ljava/lang/Object;)Ljava/lang/StringBuilder;", false, f(|e, this, a| {
        let cur = e.field_str(this, "s").unwrap_or_default();
        let v = e.arg_ref(a, 0);
        let add = if v.is_null() {
            "null".to_string()
        } else {
            {
                let sv = e.call(v, "Ljava/lang/Object;", "toString", "()Ljava/lang/String;", vec![])?;
                e.m.str_value(sv).unwrap_or_else(|| "null".to_string())
            }
        };
        let merged = format!("{cur}{add}");
        let sv = e.make_str(&merged);
        e.set_field(this, "s", sv);
        Ok(this)
    }));
    registry.register(sb, "toString", "()Ljava/lang/String;", false, f(|e, this, _a| {
        let v = e.field_str(this, "s").unwrap_or_default();
        Ok(e.make_str(&v))
    }));
    registry.register(sb, "length", "()I", false, f(|e, this, _a| {
        Ok(Value::I(e.field_str(this, "s").unwrap_or_default().chars().count() as i32))
    }));
    registry.register(sb, "reverse", "()Ljava/lang/StringBuilder;", false, f(|e, this, _a| {
        let s: String = e.field_str(this, "s").unwrap_or_default().chars().rev().collect();
        let sv = e.make_str(&s);
        e.set_field(this, "s", sv);
        Ok(this)
    }));
    registry.register(sb, "<init>", "()V", false, f(|e, this, _a| {
        let sv = e.make_str("");
        e.set_field(this, "s", sv);
        Ok(Value::null())
    }));
    registry.register(sb, "<init>", "(Ljava/lang/String;)V", false, f(|e, this, a| {
        let sv = e.make_str(&e.arg_str(a, 0).unwrap_or_default());
        e.set_field(this, "s", sv);
        Ok(Value::null())
    }));

    // ---------- boxed types ----------
    for (cls, shorty, field) in [
        ("Ljava/lang/Integer;", "(I)Ljava/lang/Integer;", "value"),
        ("Ljava/lang/Long;", "(J)Ljava/lang/Long;", "value"),
        ("Ljava/lang/Boolean;", "(Z)Ljava/lang/Boolean;", "value"),
        ("Ljava/lang/Byte;", "(B)Ljava/lang/Byte;", "value"),
        ("Ljava/lang/Character;", "(C)Ljava/lang/Character;", "value"),
        ("Ljava/lang/Short;", "(S)Ljava/lang/Short;", "value"),
        ("Ljava/lang/Float;", "(F)Ljava/lang/Float;", "value"),
        ("Ljava/lang/Double;", "(D)Ljava/lang/Double;", "value"),
    ] {
        let cls_name = cls.to_string();
        let field = field.to_string();
        registry.register(&cls_name, "valueOf", shorty, true, f(move |e, _t, a| {
            let obj = e.alloc(cls)?;
            e.set_field(obj, &field, a.first().copied().unwrap_or(Value::null()));
            Ok(obj)
        }));
    }
    registry.register("Ljava/lang/Integer;", "intValue", "()I", false, f(|e, this, _a| {
        Ok(e.get_field(this, "value").unwrap_or(Value::I(0)))
    }));
    registry.register("Ljava/lang/Long;", "longValue", "()J", false, f(|e, this, _a| {
        Ok(e.get_field(this, "value").unwrap_or(Value::L(0)))
    }));
    registry.register("Ljava/lang/Boolean;", "booleanValue", "()Z", false, f(|e, this, _a| {
        Ok(e.get_field(this, "value").unwrap_or(Value::I(0)))
    }));
    registry.register("Ljava/lang/Double;", "doubleValue", "()D", false, f(|e, this, _a| {
        Ok(e.get_field(this, "value").unwrap_or(Value::D(0.0)))
    }));
    registry.register("Ljava/lang/Float;", "floatValue", "()F", false, f(|e, this, _a| {
        Ok(e.get_field(this, "value").unwrap_or(Value::F(0.0)))
    }));
    registry.register("Ljava/lang/Integer;", "parseInt", "(Ljava/lang/String;)I", true, f(|e, _t, a| {
        let s = e.arg_str(a, 0).unwrap_or_default().trim().to_string();
        match s.parse::<i32>() {
            Ok(v) => Ok(Value::I(v)),
            Err(_) => Err(e.throw("Ljava/lang/NumberFormatException;", format!("For input string: \"{s}\""))),
        }
    }));
    registry.register("Ljava/lang/Long;", "parseLong", "(Ljava/lang/String;)J", true, f(|e, _t, a| {
        let s = e.arg_str(a, 0).unwrap_or_default().trim().to_string();
        match s.parse::<i64>() {
            Ok(v) => Ok(Value::L(v)),
            Err(_) => Err(e.throw("Ljava/lang/NumberFormatException;", format!("For input string: \"{s}\""))),
        }
    }));
    registry.register("Ljava/lang/Double;", "parseDouble", "(Ljava/lang/String;)D", true, f(|e, _t, a| {
        let s = e.arg_str(a, 0).unwrap_or_default().trim().to_string();
        match s.parse::<f64>() {
            Ok(v) => Ok(Value::D(v)),
            Err(_) => Err(e.throw("Ljava/lang/NumberFormatException;", format!("For input string: \"{s}\""))),
        }
    }));
    registry.register("Ljava/lang/Integer;", "toString", "(I)Ljava/lang/String;", true, f(|e, _t, a| {
        Ok(e.make_str(&e.arg_i(a, 0).to_string()))
    }));
    registry.register("Ljava/lang/Integer;", "toString", "(II)Ljava/lang/String;", true, f(|e, _t, a| {
        let v = e.arg_i(a, 0);
        let radix = e.arg_i(a, 1).clamp(2, 36) as u32;
        Ok(e.make_str(&format_radix(v as u32, radix)))
    }));
    registry.register("Ljava/lang/Integer;", "toHexString", "(I)Ljava/lang/String;", true, f(|e, _t, a| {
        Ok(e.make_str(&format!("{:x}", e.arg_i(a, 0))))
    }));
    registry.register("Ljava/lang/Long;", "toString", "(J)Ljava/lang/String;", true, f(|e, _t, a| {
        Ok(e.make_str(&e.arg_l(a, 0).to_string()))
    }));
    registry.register("Ljava/lang/Integer;", "compare", "(II)I", true, f(|_e, _t, a| {
        let x = _e.arg_i(a, 0);
        let y = _e.arg_i(a, 1);
        Ok(Value::I(if x < y { -1 } else if x > y { 1 } else { 0 }))
    }));
    registry.register("Ljava/lang/Integer;", "equals", "(Ljava/lang/Object;)Z", false, f(|e, this, a| {
        let mine = e.get_field(this, "value").unwrap_or(Value::I(0));
        let other = e.arg_ref(a, 0);
        let same = other != Value::null() && e.get_field(other, "value") == Some(mine);
        Ok(Value::I(same as i32))
    }));

    // static finals for boxed types (read via sget on shim statics)
    // registered at boot in statics (see register_statics)

    // ---------- java/lang/Math ----------
    registry.register("Ljava/lang/Math;", "abs", "(I)I", true, f(|e, _t, a| Ok(Value::I(e.arg_i(a, 0).wrapping_abs()))));
    registry.register("Ljava/lang/Math;", "abs", "(J)J", true, f(|e, _t, a| Ok(Value::L(e.arg_l(a, 0).wrapping_abs()))));
    registry.register("Ljava/lang/Math;", "abs", "(F)F", true, f(|e, _t, a| Ok(Value::F(e.arg_f(a, 0).abs()))));
    registry.register("Ljava/lang/Math;", "abs", "(D)D", true, f(|e, _t, a| Ok(Value::D(e.arg_d(a, 0).abs()))));
    registry.register("Ljava/lang/Math;", "min", "(II)I", true, f(|e, _t, a| Ok(Value::I(e.arg_i(a, 0).min(e.arg_i(a, 1))))));
    registry.register("Ljava/lang/Math;", "max", "(II)I", true, f(|e, _t, a| Ok(Value::I(e.arg_i(a, 0).max(e.arg_i(a, 1))))));
    registry.register("Ljava/lang/Math;", "min", "(JJ)J", true, f(|e, _t, a| Ok(Value::L(e.arg_l(a, 0).min(e.arg_l(a, 1))))));
    registry.register("Ljava/lang/Math;", "max", "(JJ)J", true, f(|e, _t, a| Ok(Value::L(e.arg_l(a, 0).max(e.arg_l(a, 1))))));
    registry.register("Ljava/lang/Math;", "sqrt", "(D)D", true, f(|e, _t, a| Ok(Value::D(e.arg_d(a, 0).sqrt()))));
    registry.register("Ljava/lang/Math;", "pow", "(DD)D", true, f(|e, _t, a| Ok(Value::D(e.arg_d(a, 0).powf(e.arg_d(a, 1))))));
    registry.register("Ljava/lang/Math;", "floor", "(D)D", true, f(|e, _t, a| Ok(Value::D(e.arg_d(a, 0).floor()))));
    registry.register("Ljava/lang/Math;", "ceil", "(D)D", true, f(|e, _t, a| Ok(Value::D(e.arg_d(a, 0).ceil()))));
    registry.register("Ljava/lang/Math;", "round", "(F)I", true, f(|e, _t, a| Ok(Value::I((e.arg_f(a, 0) + 0.5).floor() as i32))));
    registry.register("Ljava/lang/Math;", "round", "(D)J", true, f(|e, _t, a| Ok(Value::L((e.arg_d(a, 0) + 0.5).floor() as i64))));
    registry.register("Ljava/lang/Math;", "random", "()D", true, f(|e, _t, _a| Ok(Value::D(e.m.rng.next_f64()))));

    // ---------- java/lang/System ----------
    registry.register("Ljava/lang/System;", "arraycopy", "(Ljava/lang/Object;ILjava/lang/Object;II)V", true, f(|e, _t, a| {
        let src = e.arg_ref(a, 0);
        let src_pos = e.arg_i(a, 1) as usize;
        let dst = e.arg_ref(a, 2);
        let dst_pos = e.arg_i(a, 3) as usize;
        let len = e.arg_i(a, 4) as usize;
        if src.is_null() || dst.is_null() {
            return Err(e.throw_npe("arraycopy null"));
        }
        let src_vals = e.m.heap.get(src.as_ref()).and_then(|d| match d {
            litebox_dalvik::ObjectData::Array { data, .. } => Some(data.clone()),
            _ => None,
        });
        let Some(src_vals) = src_vals else { return Err(e.throw("Ljava/lang/ArrayStoreException;", "not an array")) };
        if src_pos + len > src_vals.len() {
            return Err(e.throw("Ljava/lang/ArrayIndexOutOfBoundsException;", format!("src {src_pos}+{len} > {}", src_vals.len())));
        }
        let Some(litebox_dalvik::ObjectData::Array { data: dst_data, .. }) = e.m.heap.get_mut(dst.as_ref()) else {
            return Err(e.throw("Ljava/lang/ArrayStoreException;", "not an array"));
        };
        if dst_pos + len > dst_data.len() {
            let n = dst_data.len();
            return Err(e.throw("Ljava/lang/ArrayIndexOutOfBoundsException;", format!("dst {dst_pos}+{len} > {n}")));
        }
        for k in 0..len {
            dst_data[dst_pos + k] = src_vals[src_pos + k];
        }
        Ok(Value::null())
    }));
    registry.register("Ljava/lang/System;", "currentTimeMillis", "()J", true, f(|e, _t, _a| {
        Ok(Value::L(e.now_millis() as i64))
    }));
    registry.register("Ljava/lang/System;", "nanoTime", "()J", true, f(|e, _t, _a| {
        Ok(Value::L(e.nano_time()))
    }));
    registry.register("Ljava/lang/System;", "identityHashCode", "(Ljava/lang/Object;)I", true, f(|_e, _t, a| {
        let v = _e.arg_ref(a, 0);
        Ok(Value::I(match v { Value::R(r) if r.idx != u32::MAX => r.idx as i32, _ => 0 }))
    }));
    registry.register("Ljava/io/PrintStream;", "println", "(Ljava/lang/String;)V", false, f(|e, _t, a| {
        let s = e.arg_str(a, 0).unwrap_or_default();
        e.log_i("System.out", &s);
        Ok(Value::null())
    }));
    registry.register("Ljava/io/PrintStream;", "println", "(Ljava/lang/Object;)V", false, f(|e, _t, a| {
        let v = e.arg_ref(a, 0);
        let s = if v.is_null() { "null".to_string() } else {
            let sv = e.call(v, "Ljava/lang/Object;", "toString", "()Ljava/lang/String;", vec![])?;
            e.m.str_value(sv).unwrap_or_else(|| "null".to_string())
        };
        e.log_i("System.out", &s);
        Ok(Value::null())
    }));
    registry.register("Ljava/io/PrintStream;", "println", "(I)V", false, f(|e, _t, a| {
        e.log_i("System.out", &e.arg_i(a, 0).to_string());
        Ok(Value::null())
    }));
    registry.register("Ljava/io/PrintStream;", "println", "(J)V", false, f(|e, _t, a| {
        e.log_i("System.out", &e.arg_l(a, 0).to_string());
        Ok(Value::null())
    }));
    registry.register("Ljava/io/PrintStream;", "println", "(Z)V", false, f(|e, _t, a| {
        e.log_i("System.out", if e.arg_i(a, 0) != 0 { "true" } else { "false" });
        Ok(Value::null())
    }));
    registry.register("Ljava/io/PrintStream;", "print", "(Ljava/lang/String;)V", false, f(|e, _t, a| {
        let s = e.arg_str(a, 0).unwrap_or_default();
        e.log_i("System.out", &s);
        Ok(Value::null())
    }));

    // ---------- throwables ----------
    for (cls, msg_ct) in [
        ("Ljava/lang/Throwable;", true),
        ("Ljava/lang/Exception;", true),
        ("Ljava/lang/Error;", true),
        ("Ljava/lang/RuntimeException;", true),
        ("Ljava/lang/NullPointerException;", true),
        ("Ljava/lang/IllegalArgumentException;", true),
        ("Ljava/lang/IllegalStateException;", true),
        ("Ljava/lang/UnsupportedOperationException;", true),
        ("Ljava/lang/ArithmeticException;", true),
        ("Ljava/lang/IndexOutOfBoundsException;", true),
        ("Ljava/lang/ClassNotFoundException;", true),
        ("Ljava/io/IOException;", true),
        ("Llitebox/UnsupportedApiException;", true),
    ] {
        let cls = cls.to_string();
        registry.register(&cls, "getMessage", "()Ljava/lang/String;", false, f(move |e, this, _a| {
            Ok(e.get_field(this, "detailMessage").unwrap_or(Value::null()))
        }));
        registry.register(&cls, "toString", "()Ljava/lang/String;", false, f(move |e, this, _a| {
            let cls_name = e.class_of(this).map(|c| e.m.registry.class_name(c).to_string()).unwrap_or_default();
            let msg = e.field_str(this, "detailMessage").unwrap_or_default();
            let full = if msg.is_empty() { cls_name } else { format!("{cls_name}: {msg}") };
            Ok(e.make_str(&full))
        }));
        registry.register(&cls, "printStackTrace", "()V", false, f(move |e, this, _a| {
            let cls_name = e.class_of(this).map(|c| e.m.registry.class_name(c).to_string()).unwrap_or_default();
            let msg = e.field_str(this, "detailMessage").unwrap_or_default();
            e.log_e("System.err", &format!("{cls_name}: {msg}"));
            Ok(Value::null())
        }));
        registry.register(&cls, "getCause", "()Ljava/lang/Throwable;", false, f(move |e, this, _a| {
            Ok(e.get_field(this, "cause").unwrap_or(Value::null()))
        }));
        let cls2 = cls.clone();
        registry.register(&cls, "<init>", "()V", false, f(move |_e, _t, _a| {
            let _ = cls2.clone();
            Ok(Value::null())
        }));
        let cls2 = cls.clone();
        registry.register(&cls, "<init>", "(Ljava/lang/String;)V", false, f(move |e, this, a| {
            let _ = cls2.clone();
            e.set_field(this, "detailMessage", e.arg_ref(a, 0));
            Ok(Value::null())
        }));
        let _ = msg_ct;
    }

    // ---------- collections ----------
    crate::collections::register(registry);

    // ---------- threads ----------
    crate::threads::register(registry);

    // ---------- java/util/Random ----------
    registry.register("Ljava/util/Random;", "<init>", "()V", false, f(|e, this, _a| {
        let seed = e.m.rng.next_u64() as i64;
        e.set_field(this, "seed", Value::L(seed));
        Ok(Value::null())
    }));
    registry.register("Ljava/util/Random;", "<init>", "(J)V", false, f(|e, this, a| {
        e.set_field(this, "seed", Value::L(e.arg_l(a, 0)));
        Ok(Value::null())
    }));
    registry.register("Ljava/util/Random;", "nextInt", "()I", false, f(|e, _t, _a| {
        Ok(Value::I(e.m.rng.next_i32()))
    }));
    registry.register("Ljava/util/Random;", "nextInt", "(I)I", false, f(|e, _t, a| {
        let bound = e.arg_i(a, 0);
        if bound <= 0 {
            return Err(e.throw("Ljava/lang/IllegalArgumentException;", "bound must be positive"));
        }
        Ok(Value::I((e.m.rng.next_u64() % bound as u64) as i32))
    }));
    registry.register("Ljava/util/Random;", "nextDouble", "()D", false, f(|e, _t, _a| {
        Ok(Value::D(e.m.rng.next_f64()))
    }));

    // ---------- java/util/Objects & Arrays ----------
    registry.register("Ljava/util/Objects;", "requireNonNull", "(Ljava/lang/Object;)Ljava/lang/Object;", true, f(|e, _t, a| {
        let v = e.arg_ref(a, 0);
        if v.is_null() {
            return Err(e.throw_npe("specified object is null"));
        }
        Ok(v)
    }));
    registry.register("Ljava/util/Objects;", "isNull", "(Ljava/lang/Object;)Z", true, f(|e, _t, a| {
        Ok(Value::I(e.arg_ref(a, 0).is_null() as i32))
    }));
    registry.register("Ljava/util/Arrays;", "toString", "([I)Ljava/lang/String;", true, f(|e, _t, a| {
        let arr = e.arg_ref(a, 0);
        let vals = e.m.heap.get(arr.as_ref()).and_then(|d| match d {
            litebox_dalvik::ObjectData::Array { data, .. } => Some(
                data.iter().map(|v| v.as_i32().to_string()).collect::<Vec<_>>().join(", ")
            ),
            _ => None,
        });
        Ok(e.make_str(&format!("[{}]", vals.unwrap_or_default())))
    }));
    registry.register("Ljava/util/Arrays;", "sort", "([I)V", true, f(|e, _t, a| {
        let arr = e.arg_ref(a, 0);
        let Some(litebox_dalvik::ObjectData::Array { data, .. }) = e.m.heap.get_mut(arr.as_ref()) else {
            return Ok(Value::null());
        };
        let mut vals: Vec<i32> = data.iter().map(|v| v.as_i32()).collect();
        vals.sort_unstable();
        for (i, v) in vals.into_iter().enumerate() {
            data[i] = Value::I(v);
        }
        Ok(Value::null())
    }));
    registry.register("Ljava/util/Arrays;", "fill", "([II)V", true, f(|e, _t, a| {
        let arr = e.arg_ref(a, 0);
        let v = Value::I(e.arg_i(a, 1));
        let Some(litebox_dalvik::ObjectData::Array { data, .. }) = e.m.heap.get_mut(arr.as_ref()) else {
            return Ok(Value::null());
        };
        for slot in data.iter_mut() {
            *slot = v;
        }
        Ok(Value::null())
    }));
    registry.register("Ljava/util/Arrays;", "asList", "([Ljava/lang/Object;)Ljava/util/List;", true, f(|e, _t, a| {
        let arr = e.arg_ref(a, 0);
        let vals = e.m.heap.get(arr.as_ref()).and_then(|d| match d {
            litebox_dalvik::ObjectData::Array { data, .. } => Some(data.to_vec()),
            _ => None,
        }).unwrap_or_default();
        let obj = e.alloc("Ljava/util/ArrayList;")?;
        e.set_field(obj, "size", Value::I(vals.len() as i32));
        let aref = e.m.alloc_array(ArrayKind::Refs, vals.len());
        if let Some(litebox_dalvik::ObjectData::Array { data, .. }) = e.m.heap.get_mut(aref) {
            *data = vals.into_boxed_slice();
        }
        e.set_field(obj, "elementData", Value::R(aref));
        Ok(obj)
    }));

    // ---------- File (sandboxed) ----------
    registry.register("Ljava/io/File;", "<init>", "(Ljava/lang/String;)V", false, f(|e, this, a| {
        e.set_field(this, "path", e.arg_ref(a, 0));
        Ok(Value::null())
    }));
    registry.register("Ljava/io/File;", "delete", "()Z", false, f(|e, this, _a| {
        let p = e.field_str(this, "path").unwrap_or_default();
        Ok(Value::I(e.m.fs.delete(&p).unwrap_or(false) as i32))
    }));
    registry.register("Ljava/io/File;", "getPath", "()Ljava/lang/String;", false, f(|e, this, _a| {
        Ok(e.get_field(this, "path").unwrap_or(Value::null()))
    }));
    registry.register("Ljava/io/File;", "mkdirs", "()Z", false, f(|e, this, _a| {
        let p = e.field_str(this, "path").unwrap_or_default();
        Ok(Value::I(e.m.fs.mkdirs(&p).is_ok() as i32))
    }));
    registry.register("Ljava/io/File;", "exists", "()Z", false, f(|e, this, _a| {
        let p = e.field_str(this, "path").unwrap_or_default();
        Ok(Value::I(e.m.fs.exists(&p) as i32))
    }));
}

pub fn format_radix(mut v: u32, radix: u32) -> String {
    if v == 0 {
        return "0".to_string();
    }
    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut out = Vec::new();
    while v > 0 {
        out.push(DIGITS[(v % radix) as usize]);
        v /= radix;
    }
    out.reverse();
    String::from_utf8(out).unwrap()
}

pub fn java_float_to_string(v: f32) -> String {
    if v.is_nan() {
        return "NaN".into();
    }
    if v.is_infinite() {
        return if v > 0.0 { "Infinity".into() } else { "-Infinity".into() };
    }
    let s = format!("{}", v);
    if s.ends_with(".0") {
        s[..s.len() - 2].to_string()
    } else {
        s
    }
}

pub fn java_double_to_string(v: f64) -> String {
    if v.is_nan() {
        return "NaN".into();
    }
    if v.is_infinite() {
        return if v > 0.0 { "Infinity".into() } else { "-Infinity".into() };
    }
    let s = format!("{}", v);
    if s.ends_with(".0") {
        s[..s.len() - 2].to_string()
    } else {
        s
    }
}

// re-export helpers for sibling modules
