//! Thread natives: cooperative task creation, sleeps, joins, naming.

use litebox_dalvik::env::Env;
use litebox_dalvik::errors::ThrowSpec;
use litebox_dalvik::tasks::TaskState;
use litebox_dalvik::value::Value;
use litebox_dalvik::NativeFn;
use std::sync::Arc;

fn f<F>(f: F) -> NativeFn
where
    F: Fn(&mut Env, Value, &[Value]) -> Result<Value, ThrowSpec> + Send + Sync + 'static,
{
    Arc::new(f)
}

pub fn register(registry: &mut litebox_dalvik::NativeRegistry) {
    const T: &str = "Ljava/lang/Thread;";
    registry.register(T, "<init>", "()V", false, f(|e, this, _a| {
        let nv = e.make_str(&format!("thread-{}", e.m.tasks.next_id));
        e.set_field(this, "name", nv);
        Ok(Value::null())
    }));
    registry.register(T, "<init>", "(Ljava/lang/String;)V", false, f(|e, this, a| {
        e.set_field(this, "name", e.arg_ref(a, 0));
        Ok(Value::null())
    }));
    registry.register(T, "<init>", "(Ljava/lang/Runnable;)V", false, f(|e, this, a| {
        e.set_field(this, "target", e.arg_ref(a, 0));
        let nv = e.make_str(&format!("thread-{}", e.m.tasks.next_id));
        e.set_field(this, "name", nv);
        Ok(Value::null())
    }));
    registry.register(T, "<init>", "(Ljava/lang/Runnable;Ljava/lang/String;)V", false, f(|e, this, a| {
        e.set_field(this, "target", e.arg_ref(a, 0));
        e.set_field(this, "name", e.arg_ref(a, 1));
        Ok(Value::null())
    }));
    registry.register(T, "start", "()V", false, f(|e, this, _a| {
        let target = e.get_field(this, "target").unwrap_or(Value::null());
        if target.is_null() {
            return Err(e.throw_npe("Thread.start without a Runnable target"));
        }
        let name = e.field_str(this, "name").unwrap_or_else(|| "thread".into());
        // resolve Runnable.run on the target's class
        let cid = e.class_of(target).ok_or_else(|| e.throw_npe("target has no class"))?;
        let nid = e.m.registry.name_id("run");
        let sid = e.m.registry.intern_shorty("V");
        let gidx = e.m.registry.classes[cid as usize]
            .vmap
            .get(&(nid.unwrap_or(u32::MAX), sid))
            .copied();
        let Some(gidx) = gidx else {
            return Err(e.throw("Ljava/lang/NoSuchMethodError;", format!("{}.run()V", e.m.registry.class_name(cid))));
        };
        let tid = e
            .m
            .spawn_task_method(&name, gidx, vec![target])
            .map_err(|er| e.throw("Ljava/lang/VmError;", er.to_string()))?;
        e.set_field(this, "taskId", Value::I(tid as i32));
        e.log_d("dalvik", &format!("thread '{name}' started (task {tid})"));
        Ok(Value::null())
    }));
    registry.register(T, "sleep", "(J)V", true, f(|e, _t, a| {
        let ms = e.arg_l(a, 0).max(0) as u64;
        let task = e.m.tasks.current_hint;
        if ms > 0 {
            let until = e.m.clock.now_us() + ms * 1000;
            let t = e.m.tasks.task_mut(task).unwrap();
            t.state = TaskState::Sleeping { until_us: until };
        }
        Ok(Value::null())
    }));
    registry.register(T, "sleep", "(JI)V", true, f(|e, _t, a| {
        let ms = e.arg_l(a, 0).max(0) as u64;
        let task = e.m.tasks.current_hint;
        if ms > 0 {
            let until = e.m.clock.now_us() + ms * 1000;
            let t = e.m.tasks.task_mut(task).unwrap();
            t.state = TaskState::Sleeping { until_us: until };
        }
        Ok(Value::null())
    }));
    registry.register(T, "yield", "()V", true, f(|_e, _t, _a| {
        // cooperative: give up the rest of the slice
        Ok(Value::null())
    }));
    registry.register(T, "currentThread", "()Ljava/lang/Thread;", true, f(|e, _t, _a| {
        let obj = e.alloc("Ljava/lang/Thread;")?;
        let task = e.m.tasks.current_hint;
        let name = e.m.tasks.task(task).map(|t| t.name.clone()).unwrap_or_else(|| "main".into());
        let nv = e.make_str(&name);
        e.set_field(obj, "name", nv);
        e.set_field(obj, "taskId", Value::I(task as i32));
        Ok(obj)
    }));
    registry.register(T, "getName", "()Ljava/lang/String;", false, f(|e, this, _a| {
        Ok(e.get_field(this, "name").unwrap_or(Value::null()))
    }));
    registry.register(T, "setName", "(Ljava/lang/String;)V", false, f(|e, this, a| {
        e.set_field(this, "name", e.arg_ref(a, 0));
        Ok(Value::null())
    }));
    registry.register(T, "isAlive", "()Z", false, f(|e, this, _a| {
        let tid = e.field_i32(this, "taskId").unwrap_or(-1);
        let alive = e
            .m
            .tasks
            .task(tid.max(0) as u32)
            .map(|t| t.state != TaskState::Done)
            .unwrap_or(false);
        Ok(Value::I(alive as i32))
    }));
    registry.register(T, "join", "()V", false, f(|e, this, _a| {
        let tid = e.field_i32(this, "taskId").unwrap_or(-1);
        if tid >= 0 {
            let task = e.m.tasks.current_hint;
            let t = e.m.tasks.task_mut(task).unwrap();
            t.state = TaskState::Joining { target: tid as u32 };
        }
        Ok(Value::null())
    }));
    registry.register(T, "interrupt", "()V", false, f(|_e, _t, _a| {
        // cooperative model: interrupts are no-ops (documented deviation)
        Ok(Value::null())
    }));
    registry.register(T, "setDaemon", "(Z)V", false, f(|_e, _t, _a| Ok(Value::null())));
}
