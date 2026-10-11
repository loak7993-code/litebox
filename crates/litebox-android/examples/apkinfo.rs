//! Debug utility: dump manifest facts from an APK.

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let apk_path = args.get(1).expect("usage: apkinfo <apk>");
    let bytes = std::fs::read(apk_path).unwrap();
    let ar = litebox_zip::ZipArchive::open(&bytes).unwrap();
    let axml = ar.read("AndroidManifest.xml").unwrap();
    match litebox_android::axml::parse_manifest(&axml) {
        Ok(m) => {
            println!("package: {}", m.package);
            println!("version: {:?}", m.version_name);
            println!("label: {:?}", m.label);
            println!("launcher: {:?}", m.launcher_activity);
            println!("activities: {:?}", m.activities);
        }
        Err(e) => println!("error: {e}"),
    }
}
