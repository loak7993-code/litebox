//! The LiteBox Android framework: shim class declarations, native method
//! implementations for java.* and android.*, and the view-tree renderer.

pub mod axml;
pub mod collections;
pub mod handlers;
pub mod natives_android;
pub mod natives_java;
pub mod pm;
pub mod shims;
pub mod threads;
pub mod views;

use litebox_dalvik::Machine;

/// Boot the framework: declare classes, then register natives. Must run
/// before any dex is loaded.
pub fn boot(machine: &mut Machine) {
    shims::declare_all(&mut machine.registry);
    natives_java::register_java(&mut machine.registry.natives);
    collections::register(&mut machine.registry.natives);
    threads::register(&mut machine.registry.natives);
    natives_android::register(&mut machine.registry.natives);
    // fill Build statics
    let build = machine.registry.class_by_name("Landroid/os/Build;");
    if let Some(bid) = build {
        let model = machine.registry.intern_string("LiteBox");
        let _ = model;
        set_static_str(machine, bid, "MODEL", "LiteBox");
        set_static_str(machine, bid, "MANUFACTURER", "LiteBox Emulator Project");
        set_static_str(machine, bid, "BRAND", "litebox");
    }
    let bver = machine.registry.class_by_name("Landroid/os/Build$VERSION;");
    if let Some(vid) = bver {
        set_static_str(machine, vid, "RELEASE", "14");
    }
}

fn set_static_str(machine: &mut Machine, class_id: u32, name: &str, v: &str) {
    let nid = machine.registry.name_id(name);
    let Some(nid) = nid else { return };
    let s = machine.make_str(v);
    if let Some(st) = machine.registry.classes[class_id as usize]
        .statics
        .iter_mut()
        .find(|s2| s2.name == nid)
    {
        st.value = s;
    }
}

/// Render the top activity's view tree into the framebuffer. Called by the
/// screenshot API (agent `screen_png`) before encoding.
pub fn render_screen(machine: &mut Machine) -> bool {
    // find the most recent activity instance on the heap (latest allocations
    // are the top of the activity stack in practice)
    let mut top: Option<litebox_dalvik::ObjRef> = None;
    for (idx, obj) in machine.heap.iter_slots() {
        if let litebox_dalvik::ObjectData::Instance { class, .. } = obj {
            let name = machine.registry.class_name(*class).to_string();
            if (name.starts_with("Landroid/app/Activity") || name.ends_with("Activity;"))
                && !name.contains("$") {
                    top = Some(litebox_dalvik::ObjRef { idx, gen: 0 });
                }
        }
    }
    let Some(activity) = top else { return false };
    let root = machine.get_instance_field(activity, "contentView").unwrap_or(litebox_dalvik::Value::null());
    if root.is_null() {
        return false;
    }
    machine.screen.clear([16, 17, 19, 255]);
    let mut env = litebox_dalvik::Env { m: machine };
    views::render(&mut env, root, 8, 8);
    true
}

/// Install-time helpers used by the package manager.
pub mod pm_support {
    use litebox_dalvik::Machine;

    /// Create a fresh Activity instance for `class_desc`, set its context
    /// fields, and run onCreate + onResume. Returns the instance ref value.
    pub fn launch_activity(
        machine: &mut Machine,
        class_desc: &str,
        pkg: &str,
    ) -> Result<litebox_dalvik::Value, litebox_dalvik::ThrowSpec> {
        let activity = machine.alloc_instance_named(class_desc)?;
        let v = litebox_dalvik::Value::R(activity);
        let pkg_v = machine.make_str(pkg);
        machine.set_instance_field(activity, "packageName", pkg_v);
        let mut env = litebox_dalvik::Env { m: machine };
        env.call(
            v,
            class_desc,
            "onCreate",
            "(Landroid/os/Bundle;)V",
            vec![litebox_dalvik::Value::null()],
        )?;
        env.call(v, class_desc, "onResume", "()V", vec![])?;
        Ok(v)
    }
}
