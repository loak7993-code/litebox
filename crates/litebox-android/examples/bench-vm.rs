//! Interpreter throughput benchmark: fib(22) recursion, array loops, string
//! building — measures dispatch overhead, invoke cost, and allocation paths.
//! Run with: cargo bench -p litebox-dalvik (or --example bench-vm).

use litebox_android::pm::{load_into, PackageManager};
use litebox_dalvik::{Env, Machine, Value};
use litebox_core::Config;
use std::path::PathBuf;
use std::time::Instant;

fn bench(name: &str, m: &mut Machine, class: &str, method: &str, sig: &str, args: Vec<Value>) {
    let t0 = Instant::now();
    let before = m.stats.instructions;
    let mut env = Env { m };
    let result = env.call_static(class, method, sig, args);
    let dt = t0.elapsed();
    let steps = m.stats.instructions - before;
    match result {
        Ok(v) => {
            println!(
                "{name:<22} {:>4.1} ms   {:>10} instr   {:>9.0} instr/s   result={}",
                dt.as_secs_f64() * 1000.0,
                steps,
                steps as f64 / dt.as_secs_f64(),
                match v {
                    Value::I(x) => x.to_string(),
                    Value::L(x) => x.to_string(),
                    _ => "?".into(),
                }
            );
        }
        Err(spec) => println!("{name:<22} FAILED: {} {}", spec.class, spec.msg),
    }
}

fn main() {
    let home = format!("/tmp/litebox-bench-{}", std::process::id());
    let cfg = Config {
        home: PathBuf::from(&home),
        ..Config::default()
    };
    let mut m = Machine::new(&cfg);
    litebox_android::boot(&mut m);
    let pm = PackageManager::new(cfg.packages_dir());
    let bytes =
        std::fs::read("/root/litebox/examples/smoke/build/smoke.apk").expect("build smoke apk first");
    let pkg = pm.install_bytes(&bytes, "bench".into(), 0).unwrap();
    load_into(&mut m, &pkg).unwrap();
    let main_cls = "Lcom/litebox/smoke/Main;";

    // warmup
    let mut env = Env { m: &mut m };
    let _ = env.call_static(main_cls, "fib", "(I)I", vec![Value::I(5)]);
    let _ = env;

    println!("litebox interpreter benchmark (v{})", env!("CARGO_PKG_VERSION"));
    bench("fib(22) recursion", &mut m, main_cls, "fib", "(I)I", vec![Value::I(22)]);
    // 10k-element int array
    let arr = m.alloc_array(litebox_dalvik::ArrayKind::Ints, 10_000);
    if let Some(litebox_dalvik::ObjectData::Array { data, .. }) = m.heap.get_mut(arr) {
        for (i, v) in data.iter_mut().enumerate() {
            *v = Value::I((i % 100) as i32);
        }
    }
    bench("sum(array 1..10k)", &mut m, main_cls, "sum", "([I)I", vec![Value::R(arr)]);
    let name = m.make_str("bench");
    bench("greet (sb chain)", &mut m, main_cls, "greet", "(Ljava/lang/String;)Ljava/lang/String;", vec![name]);
    bench("wideOps", &mut m, main_cls, "wideOps", "(JJ)J", vec![Value::L(0x1234), Value::L(0x9abc)]);
    bench("switchDay(3)", &mut m, main_cls, "switchDay", "(I)Ljava/lang/String;", vec![Value::I(3)]);

    let total = m.stats.instructions;
    println!(
        "\ntotal instructions this run: {} (gc cycles: {})",
        total, m.heap.gc_count
    );
}
