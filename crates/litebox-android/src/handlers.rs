//! Handler/timer scheduling: posting Runnables onto the deterministic
//! virtual-clock queue. A posted runnable becomes a sleeping task that the
//! scheduler wakes at its due time.

use litebox_dalvik::env::Env;
use litebox_dalvik::errors::ThrowSpec;
use litebox_dalvik::tasks::TaskState;
use litebox_dalvik::value::Value;

/// Schedule `runnable.run()` at now+delay (virtual ms).
pub fn post_runnable(e: &mut Env, runnable: Value, delay_ms: u64) -> Result<(), ThrowSpec> {
    if runnable.is_null() {
        return Err(e.throw_npe("post(null)"));
    }
    let cid = e.class_of(runnable).ok_or_else(|| e.throw_npe("runnable has no class"))?;
    let nid = e.m.registry.name_id("run");
    let sid = e.m.registry.intern_shorty("V");
    let gidx = e.m.registry.classes[cid as usize]
        .vmap
        .get(&(nid.unwrap_or(u32::MAX), sid))
        .copied();
    let Some(gidx) = gidx else {
        return Err(e.throw("Ljava/lang/NoSuchMethodError;", format!("{}.run()V", e.m.registry.class_name(cid))));
    };
    let name = format!("handler-{}", e.m.tasks.next_id);
    let tid = e
        .m
        .spawn_task_method(&name, gidx, vec![runnable])
        .map_err(|er| e.throw("Ljava/lang/VmError;", er.to_string()))?;
    if delay_ms > 0 {
        let until = e.m.clock.now_us() + delay_ms * 1000;
        let t = e.m.tasks.task_mut(tid).unwrap();
        t.state = TaskState::Sleeping { until_us: until };
    }
    Ok(())
}
