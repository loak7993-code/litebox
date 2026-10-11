//! Native method implementations for android.* framework classes.


use litebox_dalvik::env::Env;
use litebox_dalvik::errors::ThrowSpec;
use litebox_dalvik::value::Value;
use litebox_dalvik::NativeFn;
use litebox_core::EventLevel;
use std::sync::Arc;

fn f<F>(f: F) -> NativeFn
where
    F: Fn(&mut Env, Value, &[Value]) -> Result<Value, ThrowSpec> + Send + Sync + 'static,
{
    Arc::new(f)
}

pub fn register(registry: &mut litebox_dalvik::NativeRegistry) {
    // ---------- android/util/Log ----------
    const LOG: &str = "Landroid/util/Log;";
    for (name, level) in [
        ("v", EventLevel::Debug),
        ("d", EventLevel::Debug),
        ("i", EventLevel::Info),
        ("w", EventLevel::Warn),
        ("e", EventLevel::Error),
    ] {
        let lvl = level;
        registry.register(LOG, name, "(Ljava/lang/String;Ljava/lang/String;)I", true, f(move |e, _t, a| {
            let tag = e.arg_str(a, 0).unwrap_or_default();
            let msg = e.arg_str(a, 1).unwrap_or_default();
            e.log(lvl, &tag, &msg);
            Ok(Value::I(msg.len() as i32))
        }));
        let lvl = level;
        registry.register(LOG, name, "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/Throwable;)I", true, f(move |e, _t, a| {
            let tag = e.arg_str(a, 0).unwrap_or_default();
            let msg = e.arg_str(a, 1).unwrap_or_default();
            let th = e.arg_ref(a, 2);
            let extra = if th.is_null() {
                String::new()
            } else {
                let cls = e.class_of(th).map(|c| e.m.registry.class_name(c).to_string()).unwrap_or_default();
                let m = e.field_str(th, "detailMessage").unwrap_or_default();
                format!(" {cls}: {m}")
            };
            e.log(lvl, &tag, &format!("{msg}{extra}"));
            Ok(Value::I((msg.len() + extra.len()) as i32))
        }));
    }

    // ---------- Build statics ----------
    registry.register("Landroid/os/Build;", "statics_init", "()V", true, f(|e, _t, _a| {
        for (field, v) in [
            ("MODEL", "LiteBox"),
            ("MANUFACTURER", "LiteBox Emulator Project"),
            ("BRAND", "litebox"),
        ] {
            if let Some(cid) = e.m.registry.class_by_name("Landroid/os/Build;") {
                let s = e.make_str(v);
                let nid = e.m.registry.name_id(field);
                if let (Some(nid), Some(s)) = (nid, Some(s)) {
                    if let Some(st) = e.m.registry.classes[cid as usize].statics.iter_mut().find(|s2| s2.name == nid) {
                        st.value = s;
                    }
                }
            }
        }
        if let Some(cid) = e.m.registry.class_by_name("Landroid/os/Build$VERSION;") {
            let nid = e.m.registry.name_id("RELEASE");
            let s = e.make_str("14");
            if let (Some(nid), Some(s)) = (nid, Some(s)) {
                if let Some(st) = e.m.registry.classes[cid as usize].statics.iter_mut().find(|s2| s2.name == nid) {
                    st.value = s;
                }
            }
        }
        Ok(Value::null())
    }));

    // ---------- Context / Application / Activity ----------
    const CTX: &str = "Landroid/content/Context;";
    registry.register(CTX, "getPackageName", "()Ljava/lang/String;", false, f(|e, this, _a| {
        Ok(e.get_field(this, "packageName").unwrap_or(Value::null()))
    }));
    registry.register(CTX, "getFilesDir", "()Ljava/io/File;", false, f(|e, this, _a| {
        let pkg = e.field_str(this, "packageName").unwrap_or_else(|| "app".into());
        let path = format!("/data/data/{pkg}/files");
        let file = e.alloc("Ljava/io/File;")?;
        let pv = e.make_str(&path);
        e.set_field(file, "path", pv);
        Ok(file)
    }));
    registry.register(CTX, "getCacheDir", "()Ljava/io/File;", false, f(|e, this, _a| {
        let pkg = e.field_str(this, "packageName").unwrap_or_else(|| "app".into());
        let path = format!("/data/data/{pkg}/cache");
        let file = e.alloc("Ljava/io/File;")?;
        let pv = e.make_str(&path);
        e.set_field(file, "path", pv);
        Ok(file)
    }));
    registry.register(CTX, "getApplicationContext", "()Landroid/content/Context;", false, f(|_e, this, _a| Ok(this)));
    registry.register(CTX, "getString", "(I)Ljava/lang/String;", false, f(|e, _t, a| {
        Err(e.throw_unsupported(format!(
            "Context.getString({}) requires resource resolution: not available for this package",
            e.arg_i(a, 0)
        )))
    }));
    registry.register("Landroid/content/ContextWrapper;", "<init>", "(Landroid/content/Context;)V", false, f(|_e, _t, _a| {
        Ok(Value::null())
    }));
    registry.register("Landroid/app/Application;", "<init>", "()V", false, f(|_e, _t, _a| {
        Ok(Value::null())
    }));

    const ACT: &str = "Landroid/app/Activity;";
    registry.register(ACT, "<init>", "()V", false, f(|e, this, _a| {
        e.set_field(this, "intent", Value::null());
        Ok(Value::null())
    }));
    registry.register(ACT, "onCreate", "(Landroid/os/Bundle;)V", false, f(|e, this, _a| {
        let pkg = e.field_str(this, "packageName").unwrap_or_else(|| "?".into());
        e.log_i("activity", &format!("onCreate {pkg}"));
        Ok(Value::null())
    }));
    registry.register(ACT, "onResume", "()V", false, f(|e, this, _a| {
        let pkg = e.field_str(this, "packageName").unwrap_or_else(|| "?".into());
        e.log_i("activity", &format!("onResume {pkg}"));
        Ok(Value::null())
    }));
    registry.register(ACT, "onPause", "()V", false, f(|e, _t, _a| {
        e.log_i("activity", "onPause");
        Ok(Value::null())
    }));
    registry.register(ACT, "onDestroy", "()V", false, f(|e, _t, _a| {
        e.log_i("activity", "onDestroy");
        Ok(Value::null())
    }));
    registry.register(ACT, "setContentView", "(Landroid/view/View;)V", false, f(|e, this, a| {
        let v = e.arg_ref(a, 0);
        e.set_field(this, "contentView", v);
        e.log_d("activity", "setContentView(view)");
        Ok(Value::null())
    }));
    registry.register(ACT, "setContentView", "(I)V", false, f(|e, _t, a| {
        // resource layouts are not inflated; a stub linear container stands in
        let root = e.alloc("Landroid/widget/LinearLayout;")?;
        e.set_field(root, "orientation", Value::I(1));
        let _ = a;
        e.log_w("activity", "setContentView(int) uses a stub container (no layout inflation)");
        Ok(root)
    }));
    registry.register(ACT, "findViewById", "(I)Landroid/view/View;", false, f(|e, this, a| {
        let id = e.arg_i(a, 0);
        let root = e.get_field(this, "contentView").unwrap_or(Value::null());
        Ok(crate::views::find_by_id(e, root, id))
    }));
    registry.register(ACT, "getIntent", "()Landroid/content/Intent;", false, f(|e, this, _a| {
        Ok(e.get_field(this, "intent").unwrap_or(Value::null()))
    }));
    registry.register(ACT, "startActivity", "(Landroid/content/Intent;)V", false, f(|e, this, a| {
        let intent = e.arg_ref(a, 0);
        let target = e.field_str(intent, "targetClass").unwrap_or_default();
        if target.is_empty() {
            e.log_w("activity", "startActivity without a component class (ignored)");
            return Ok(Value::null());
        }
        // instantiate the target activity and run its lifecycle
        let activity = e.alloc(&target)?;
        e.set_field(activity, "intent", intent);
        e.set_field(activity, "packageName", e.get_field(this, "packageName").unwrap_or(Value::null()));
        e.call(activity, &target, "onCreate", "(Landroid/os/Bundle;)V", vec![Value::null()])?;
        e.call(activity, &target, "onResume", "()V", vec![])?;
        Ok(Value::null())
    }));
    registry.register(ACT, "runOnUiThread", "(Ljava/lang/Runnable;)V", false, f(|e, _t, a| {
        crate::handlers::post_runnable(e, e.arg_ref(a, 0), 0)?;
        Ok(Value::null())
    }));
    registry.register(ACT, "finish", "()V", false, f(|e, _t, _a| {
        e.log_i("activity", "finish");
        Ok(Value::null())
    }));

    // ---------- Intent / Bundle ----------
    const INT: &str = "Landroid/content/Intent;";
    registry.register(INT, "<init>", "()V", false, f(|e, this, _a| {
        let map = e.alloc("Ljava/util/HashMap;")?;
        e.set_field(this, "extras", map);
        Ok(Value::null())
    }));
    registry.register(INT, "<init>", "(Ljava/lang/String;)V", false, f(|e, this, a| {
        e.set_field(this, "action", e.arg_ref(a, 0));
        let map = e.alloc("Ljava/util/HashMap;")?;
        e.set_field(this, "extras", map);
        Ok(Value::null())
    }));
    registry.register(INT, "<init>", "(Landroid/content/Context;Ljava/lang/Class;)V", false, f(|e, this, a| {
        let map = e.alloc("Ljava/util/HashMap;")?;
        e.set_field(this, "extras", map);
        // target class name from the Class object
        let cv = e.arg_ref(a, 1);
        let cname = e.m.heap.get(cv.as_ref()).and_then(|d| match d {
            litebox_dalvik::ObjectData::Class(cid) => Some(e.m.registry.class_name(*cid).to_string()),
            _ => None,
        }).unwrap_or_default();
        let tv = e.make_str(&cname);
        e.set_field(this, "targetClass", tv);
        Ok(Value::null())
    }));
    registry.register(INT, "setClass", "(Landroid/content/Context;Ljava/lang/Class;)Landroid/content/Intent;", false, f(|e, this, a| {
        let cv = e.arg_ref(a, 1);
        let cname = e.m.heap.get(cv.as_ref()).and_then(|d| match d {
            litebox_dalvik::ObjectData::Class(cid) => Some(e.m.registry.class_name(*cid).to_string()),
            _ => None,
        }).unwrap_or_default();
        let tv = e.make_str(&cname);
        e.set_field(this, "targetClass", tv);
        Ok(this)
    }));
    registry.register(INT, "putExtra", "(Ljava/lang/String;Ljava/lang/String;)Landroid/content/Intent;", false, f(|e, this, a| {
        let extras = e.get_field(this, "extras").unwrap_or(Value::null());
        e.call(extras, "Ljava/util/HashMap;", "put", "(Ljava/lang/Object;Ljava/lang/Object;)Ljava/lang/Object;", vec![
            e.arg_ref(a, 0), e.arg_ref(a, 1),
        ])?;
        Ok(this)
    }));
    registry.register(INT, "putExtra", "(Ljava/lang/String;I)Landroid/content/Intent;", false, f(|e, this, a| {
        let extras = e.get_field(this, "extras").unwrap_or(Value::null());
        let boxed = e.alloc("Ljava/lang/Integer;")?;
        e.set_field(boxed, "value", Value::I(e.arg_i(a, 1)));
        e.call(extras, "Ljava/util/HashMap;", "put", "(Ljava/lang/Object;Ljava/lang/Object;)Ljava/lang/Object;", vec![
            e.arg_ref(a, 0), boxed,
        ])?;
        Ok(this)
    }));
    registry.register(INT, "getStringExtra", "(Ljava/lang/String;)Ljava/lang/String;", false, f(|e, this, a| {
        let extras = e.get_field(this, "extras").unwrap_or(Value::null());
        e.call(extras, "Ljava/util/HashMap;", "get", "(Ljava/lang/Object;)Ljava/lang/Object;", vec![e.arg_ref(a, 0)])
    }));
    registry.register(INT, "getIntExtra", "(Ljava/lang/String;I)I", false, f(|e, this, a| {
        let extras = e.get_field(this, "extras").unwrap_or(Value::null());
        let v = e.call(extras, "Ljava/util/HashMap;", "get", "(Ljava/lang/Object;)Ljava/lang/Object;", vec![e.arg_ref(a, 0)])?;
        if v.is_null() {
            return Ok(Value::I(e.arg_i(a, 1)));
        }
        e.call(v, "Ljava/lang/Integer;", "intValue", "()I", vec![])
    }));
    registry.register(INT, "getAction", "()Ljava/lang/String;", false, f(|e, this, _a| {
        Ok(e.get_field(this, "action").unwrap_or(Value::null()))
    }));

    const BD: &str = "Landroid/os/Bundle;";
    registry.register(BD, "<init>", "()V", false, f(|e, this, _a| {
        let map = e.alloc("Ljava/util/HashMap;")?;
        e.set_field(this, "map", map);
        Ok(Value::null())
    }));
    registry.register(BD, "putString", "(Ljava/lang/String;Ljava/lang/String;)V", false, f(|e, this, a| {
        let map = e.get_field(this, "map").unwrap_or(Value::null());
        e.call(map, "Ljava/util/HashMap;", "put", "(Ljava/lang/Object;Ljava/lang/Object;)Ljava/lang/Object;", vec![
            e.arg_ref(a, 0), e.arg_ref(a, 1),
        ])?;
        Ok(Value::null())
    }));
    registry.register(BD, "putInt", "(Ljava/lang/String;I)V", false, f(|e, this, a| {
        let map = e.get_field(this, "map").unwrap_or(Value::null());
        let boxed = e.alloc("Ljava/lang/Integer;")?;
        e.set_field(boxed, "value", Value::I(e.arg_i(a, 1)));
        e.call(map, "Ljava/util/HashMap;", "put", "(Ljava/lang/Object;Ljava/lang/Object;)Ljava/lang/Object;", vec![
            e.arg_ref(a, 0), boxed,
        ])?;
        Ok(Value::null())
    }));
    registry.register(BD, "getString", "(Ljava/lang/String;)Ljava/lang/String;", false, f(|e, this, a| {
        let map = e.get_field(this, "map").unwrap_or(Value::null());
        e.call(map, "Ljava/util/HashMap;", "get", "(Ljava/lang/Object;)Ljava/lang/Object;", vec![e.arg_ref(a, 0)])
    }));
    registry.register(BD, "getInt", "(Ljava/lang/String;I)I", false, f(|e, this, a| {
        let map = e.get_field(this, "map").unwrap_or(Value::null());
        let v = e.call(map, "Ljava/util/HashMap;", "get", "(Ljava/lang/Object;)Ljava/lang/Object;", vec![e.arg_ref(a, 0)])?;
        if v.is_null() {
            return Ok(Value::I(e.arg_i(a, 1)));
        }
        e.call(v, "Ljava/lang/Integer;", "intValue", "()I", vec![])
    }));

    // ---------- Handler / Looper ----------
    const HD: &str = "Landroid/os/Handler;";
    registry.register(HD, "<init>", "()V", false, f(|e, this, _a| {
        let pending = e.alloc("Ljava/util/ArrayList;")?;
        e.set_field(this, "pending", pending);
        Ok(Value::null())
    }));
    registry.register(HD, "<init>", "(Landroid/os/Looper;)V", false, f(|e, this, _a| {
        let pending = e.alloc("Ljava/util/ArrayList;")?;
        e.set_field(this, "pending", pending);
        Ok(Value::null())
    }));
    registry.register(HD, "post", "(Ljava/lang/Runnable;)Z", false, f(|e, _t, a| {
        crate::handlers::post_runnable(e, e.arg_ref(a, 0), 0)?;
        Ok(Value::I(1))
    }));
    registry.register(HD, "postDelayed", "(Ljava/lang/Runnable;J)Z", false, f(|e, _t, a| {
        let delay = e.arg_l(a, 1).max(0) as u64;
        crate::handlers::post_runnable(e, e.arg_ref(a, 0), delay)?;
        Ok(Value::I(1))
    }));
    registry.register(HD, "removeCallbacks", "(Ljava/lang/Runnable;)V", false, f(|_e, _t, _a| {
        // deterministic queue keeps posted tasks; removal unsupported (documented)
        Ok(Value::null())
    }));
    registry.register("Landroid/os/Looper;", "getMainLooper", "()Landroid/os/Looper;", true, f(|e, _t, _a| {
        let obj = e.alloc("Landroid/os/Looper;")?;
        Ok(obj)
    }));
    registry.register("Landroid/os/Looper;", "myLooper", "()Landroid/os/Looper;", true, f(|e, _t, _a| {
        let obj = e.alloc("Landroid/os/Looper;")?;
        Ok(obj)
    }));

    // ---------- View / TextView / Button / ViewGroup ----------
    crate::views::register(registry);
}


