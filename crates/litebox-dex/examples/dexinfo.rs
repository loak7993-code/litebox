//! Debug utility: summarize a .dex (or an APK containing classes.dex) the way
//! the runtime sees it. Usage: dexinfo <file.apk|file.dex> [descriptor]

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: dexinfo <file.apk|file.dex> [class-descriptor]");
        std::process::exit(2);
    }
    let path = &args[1];
    let filter = args.get(2).map(|s| s.as_str());
    let bytes = std::fs::read(path).expect("read file");
    let dex_bytes = if bytes.starts_with(b"PK") {
        let ar = litebox_zip::ZipArchive::open(&bytes).expect("zip");
        ar.read("classes.dex").expect("classes.dex in apk")
    } else {
        bytes
    };
    let dex = litebox_dex::Dex::parse(dex_bytes).expect("dex");
    println!(
        "strings={} types={} protos={} fields={} methods={} classes={}",
        dex.strings_count(),
        dex.type_ids.len(),
        dex.protos.len(),
        dex.field_ids.len(),
        dex.method_ids.len(),
        dex.class_defs.len()
    );
    for cd in &dex.class_defs {
        let name = dex.type_name(cd.class_idx).unwrap_or("?");
        if let Some(f) = filter {
            if !name.contains(f) {
                continue;
            }
        }
        println!(
            "class {} super={} flags={:#x} statics={} fields={} direct={} virtual={}",
            name,
            dex.type_name(cd.superclass_idx).unwrap_or("-"),
            cd.access_flags,
            cd.static_fields.len(),
            cd.instance_fields.len(),
            cd.direct_methods.len(),
            cd.virtual_methods.len()
        );
        for m in cd.direct_methods.iter().chain(cd.virtual_methods.iter()) {
            let mid = m.method_idx;
            let proto = dex.proto_of(mid).map(|p| {
                let r = dex.type_name(p.return_type_idx).unwrap_or("?");
                let ps: Vec<String> = p
                    .params
                    .iter()
                    .filter_map(|&t| dex.type_name(t).ok().map(|s| s.to_string()))
                    .collect();
                format!("({}){}", ps.join(""), r)
            });
            println!(
                "  method {} {} {} code={} regs={} insns={}",
                dex.method_name(mid).unwrap_or("?"),
                proto.as_deref().unwrap_or("?"),
                if m.access_flags & 0x100 != 0 { "abstract" } else { "" },
                m.code.is_some(),
                m.code.as_ref().map(|c| c.registers_size).unwrap_or(0),
                m.code.as_ref().map(|c| c.insns.len()).unwrap_or(0)
            );
        }
    }
}
