//! Snapshot save/restore round-trip: run halfway, serialize the whole VM,
//! restore into a fresh machine, continue, and require identical results.

use litebox_android::pm::{load_into, PackageManager};
use litebox_dalvik::snapshot;
use litebox_dalvik::{Env, Machine, Value};
use litebox_core::Config;
use std::path::PathBuf;

fn fresh_machine(home: &str) -> Machine {
    let cfg = Config {
        home: PathBuf::from(home),
        ..Config::default()
    };
    let mut m = Machine::new(&cfg);
    litebox_android::boot(&mut m);
    let pm = PackageManager::new(cfg.packages_dir());
    let bytes = std::fs::read("/root/litebox/examples/smoke/build/smoke.apk").unwrap();
    let pkg = pm.install_bytes(&bytes, "smoke".into(), 0).unwrap();
    load_into(&mut m, &pkg).unwrap();
    m
}

fn call_fib(m: &mut Machine, n: i32) -> Result<i32, String> {
    let mut env = Env { m };
    env.call_static("Lcom/litebox/smoke/Main;", "fib", "(I)I", vec![Value::I(n)])
        .map(|v| v.as_i32())
        .map_err(|spec| format!("{}: {}", spec.class, spec.msg))
}

#[test]
fn snapshot_roundtrip_preserves_execution() {
    let blob_home = format!("/tmp/litebox-snap-{}", std::process::id());
    let mut m = fresh_machine(&blob_home);

    // warm the VM: fib(5) = 5, interned strings, class state
    assert_eq!(call_fib(&mut m, 5).unwrap(), 5);
    let instr_before = m.stats.instructions;
    assert!(instr_before > 0);

    let blob = snapshot::save(&m);
    assert_eq!(&blob[..8], snapshot::MAGIC);
    let path = format!("{blob_home}/snap.bin");
    std::fs::write(&path, &blob).unwrap();

    // restore into a FRESH machine (boot + dex load happen first)
    let mut m2 = fresh_machine(&format!("{blob_home}-2"));
    snapshot::load(&mut m2, std::fs::read(&path).unwrap().as_slice()).expect("restore");

    // state carried over
    assert_eq!(m2.stats.instructions, instr_before);
    assert_eq!(m2.clock.now_us(), m.clock.now_us());

    // and execution continues correctly from the restored state
    assert_eq!(call_fib(&mut m2, 10).unwrap(), 55);
    // original machine still valid
    assert_eq!(call_fib(&mut m, 10).unwrap(), 55);
}

#[test]
fn snapshot_preserves_tasks_and_heap() {
    let home = format!("/tmp/litebox-snap2-{}", std::process::id());
    let mut m = fresh_machine(&home);

    // build objects and a list
    let list = m.alloc_instance_named("Ljava/util/ArrayList;").unwrap();
    for s in ["x", "y"] {
        let sv = m.make_str(s);
        let mut env = Env { m: &mut m };
        env.call(Value::R(list), "Ljava/util/ArrayList;", "add", "(Ljava/lang/Object;)Z", vec![sv])
            .unwrap();
    }
    let size_before = m.get_instance_field(list, "size").unwrap().as_i32();
    assert_eq!(size_before, 2);

    let blob = snapshot::save(&m);
    let mut m2 = fresh_machine(&format!("{home}-2"));
    snapshot::load(&mut m2, &blob).expect("restore");

    let size_after = m2.get_instance_field(list, "size").unwrap().as_i32();
    assert_eq!(size_after, 2, "object identity survives snapshots");
    // the list still works after restore
    let sv = m2.make_str("z");
    let mut env = Env { m: &mut m2 };
    env.call(Value::R(list), "Ljava/util/ArrayList;", "add", "(Ljava/lang/Object;)Z", vec![sv])
        .unwrap();
    let size = m2.get_instance_field(list, "size").unwrap().as_i32();
    assert_eq!(size, 3, "mutation after restore");
}
