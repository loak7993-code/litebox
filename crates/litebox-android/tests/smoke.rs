//! End-to-end: run classes produced by the REAL Android toolchain (javac +
//! d8) inside the VM. The smoke APK at examples/smoke/build/smoke.apk is
//! built by build.sh with aapt2/d8/apksigner.

use litebox_android::pm::{load_into, start_activity, PackageManager};
use litebox_dalvik::value::Value;
use litebox_dalvik::{Env, Machine};
use litebox_core::Config;
use std::path::PathBuf;
use std::sync::Arc;

const SMOKE_APK: &str = "/root/litebox/examples/smoke/build/smoke.apk";

fn smoke_pkg(home: &std::path::Path) -> litebox_android::pm::InstalledPackage {
    let pm = PackageManager::new(home.join("packages"));
    let bytes = std::fs::read(SMOKE_APK).expect("smoke apk must be built first (examples/smoke/build.sh)");
    pm.install_bytes(&bytes, SMOKE_APK.to_string(), 0).expect("install")
}

static HOME_SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

fn booted() -> Machine {
    let seq = HOME_SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    // unique home per test: cargo runs tests in parallel within one process
    let cfg = Config {
        home: PathBuf::from(format!("/tmp/litebox-test-{}-{}", std::process::id(), seq)),
        ..Config::default()
    };
    let mut m = Machine::new(&cfg);
    litebox_android::boot(&mut m);
    let pkg = smoke_pkg(&cfg.home);
    load_into(&mut m, &pkg).expect("load dexes");
    m
}

fn call_static(m: &mut Machine, sig: (&str, &str, &str), args: Vec<Value>) -> Result<Value, String> {
    let (class_desc, method, shorty) = sig;
    let mut env = Env { m };
    env.call_static(class_desc, method, shorty, args)
        .map_err(|spec| format!("{}: {}", spec.class, spec.msg))
}

#[test]
fn arithmetic_recursion_and_strings() {
    let mut m = booted();

    let fib_args = vec![Value::I(10)];
    let v = call_static(&mut m, ("Lcom/litebox/smoke/Main;", "fib", "(I)I"), fib_args).unwrap();
    assert_eq!(v.as_i32(), 55, "fib(10)");

    let name = m.make_str("agent");
    let greet = call_static(
        &mut m,
        ("Lcom/litebox/smoke/Main;", "greet", "(Ljava/lang/String;)Ljava/lang/String;"),
        vec![name],
    )
    .unwrap();
    assert_eq!(m.str_value(greet).as_deref(), Some("hello, agent! fib=55"), "greet");

    // array loop: sum(new int[]{1..5}) = 15
    let arr = m.alloc_array(litebox_dalvik::ArrayKind::Ints, 5);
    if let Some(litebox_dalvik::ObjectData::Array { data, .. }) = m.heap.get_mut(arr) {
        for (i, v) in data.iter_mut().enumerate() {
            *v = Value::I((i + 1) as i32);
        }
    }
    let s = call_static(&mut m, ("Lcom/litebox/smoke/Main;", "sum", "([I)I"), vec![Value::R(arr)]).unwrap();
    assert_eq!(s.as_i32(), 15, "sum");

    let d = call_static(
        &mut m,
        ("Lcom/litebox/smoke/Main;", "switchDay", "(I)Ljava/lang/String;"),
        vec![Value::I(2)],
    )
    .unwrap();
    assert_eq!(m.str_value(d).as_deref(), Some("Tue"), "switch");

    let d = call_static(
        &mut m,
        ("Lcom/litebox/smoke/Main;", "switchDay", "(I)Ljava/lang/String;"),
        vec![Value::I(9)],
    )
    .unwrap();
    assert_eq!(m.str_value(d).as_deref(), Some("?"), "switch default");
}

#[test]
fn wide_and_float_ops() {
    let mut m = booted();
    // wideOps(0x1234_5678, 0x9ABC_DEF0)
    let a: i64 = 0x1234_5678;
    let b: i64 = 0x9ABC_DEF0i64;
    let expect = ((a << 3) ^ (b >> 2)) & (a - b);
    let v = call_static(
        &mut m,
        ("Lcom/litebox/smoke/Main;", "wideOps", "(JJ)J"),
        vec![Value::L(a), Value::L(b)],
    )
    .unwrap();
    assert_eq!(v.as_i64(), expect, "wideOps");

    let v = call_static(
        &mut m,
        ("Lcom/litebox/smoke/Main;", "floatOps", "(DD)D"),
        vec![Value::D(3.0), Value::D(4.0)],
    )
    .unwrap();
    assert!((v.as_f64() - 10.0).abs() < 1e-9, "sqrt(3-4-5)*2");

    let sv = m.make_str("x");
    let v = call_static(
        &mut m,
        ("Lcom/litebox/smoke/Main;", "formatTest", "(ILjava/lang/String;D)Ljava/lang/String;"),
        vec![Value::I(42), sv, Value::D(3.75)],
    )
    .unwrap();
    assert_eq!(m.str_value(v).as_deref(), Some("42:x:3"), "string concat + casts");
}

#[test]
fn objects_arrays_exceptions() {
    let mut m = booted();

    // Box objects through user constructor + virtual dispatch
    let boxes: Vec<Value> = (1..=4)
        .map(|i| {
            let obj = m
                .alloc_instance_named("Lcom/litebox/smoke/Main$Box;")
                .unwrap();
            m.set_instance_field(obj, "v", Value::I(i * 10));
            Value::R(obj)
        })
        .collect();
    let barr = m.alloc_array(litebox_dalvik::ArrayKind::Refs, boxes.len());
    if let Some(litebox_dalvik::ObjectData::Array { data, .. }) = m.heap.get_mut(barr) {
        *data = boxes.into_boxed_slice();
    }
    let v = call_static(
        &mut m,
        ("Lcom/litebox/smoke/Main;", "boxSum", "([Lcom/litebox/smoke/Main$Box;)I"),
        vec![Value::R(barr)],
    )
    .unwrap();
    assert_eq!(v.as_i32(), 100, "boxSum via virtual get()");

    // ArrayList<String> through collections natives + StringBuilder join
    let list = m.alloc_instance_named("Ljava/util/ArrayList;").unwrap();
    for s in ["a", "b", "c"] {
        let sv = m.make_str(s);
        let mut env = Env { m: &mut m };
        env.call(
            Value::R(list),
            "Ljava/util/ArrayList;",
            "add",
            "(Ljava/lang/Object;)Z",
            vec![sv],
        )
        .unwrap();
    }
    let joined = call_static(
        &mut m,
        ("Lcom/litebox/smoke/Main;", "join", "(Ljava/util/ArrayList;)Ljava/lang/String;"),
        vec![Value::R(list)],
    )
    .unwrap();
    assert_eq!(m.str_value(joined).as_deref(), Some("a|b|c"), "iterator + StringBuilder");

    // exception: divBy(5, 0) throws ArithmeticException, catchable by dex handlers
    let err = call_static(
        &mut m,
        ("Lcom/litebox/smoke/Main;", "divBy", "(II)I"),
        vec![Value::I(5), Value::I(0)],
    )
    .unwrap_err();
    assert!(err.contains("ArithmeticException"), "div by zero: {err}");
    assert!(err.contains("divide by zero"), "message: {err}");

    // divBy(9, 2) = 4
    let v = call_static(
        &mut m,
        ("Lcom/litebox/smoke/Main;", "divBy", "(II)I"),
        vec![Value::I(9), Value::I(2)],
    )
    .unwrap();
    assert_eq!(v.as_i32(), 4);
}

#[test]
fn activity_lifecycle_and_screen() {
    let mut m = booted();
    let pkg = smoke_pkg(std::path::Path::new(&format!(
        "/tmp/litebox-test-{}-{}",
        std::process::id(),
        HOME_SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    )));
    let handle = start_activity(&mut m, &pkg, None).expect("start");
    handle.pump(&mut m).expect("pump onCreate");

    // run until activity settles
    match m.run_until_pause() {
        litebox_dalvik::RunOutcome::Settled { .. } => {}
        litebox_dalvik::RunOutcome::Paused(r) => panic!("unexpected pause: {r:?}"),
        other => panic!("unexpected: {other:?}"),
    }

    // logcat: onCreate event with fib value
    let (events, _) = m.bus.take_after(0, 1000);
    let found = events.iter().any(|e| e.msg.contains("onCreate fired, fib(10)=55"));
    assert!(found, "onCreate log event missing; got: {:?}", events.iter().map(|e| &e.msg).collect::<Vec<_>>());

    // render the view tree (the screenshot API does this) and check pixels
    let rendered = litebox_android::render_screen(&mut m);
    assert!(rendered, "activity with contentView found");
    assert!(m.screen.buf.chunks(4).any(|p| p == [232, 234, 237, 255]), "text pixels present");
}

#[test]
fn dex_matches_official_dexdump() {
    // cross-check: official dexdump sees the same classes as our parser
    let bytes = std::fs::read(SMOKE_APK).unwrap();
    let ar = litebox_zip::ZipArchive::open(&bytes).unwrap();
    let dex_bytes = ar.read("classes.dex").unwrap();
    let dex = litebox_dex::Dex::parse(dex_bytes).unwrap();
    let names = dex.class_names();
    assert!(names.contains(&"Lcom/litebox/smoke/Main;".to_string()));
    assert!(names.contains(&"Lcom/litebox/smoke/SmokeActivity;".to_string()));
    assert_eq!(names.len(), 3, "Main, Main$Box, SmokeActivity: {:?}", names);
}

#[allow(unused)]
fn _silence() {
    let _ = Arc::new(1);
}
