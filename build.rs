use std::path::Path;

fn main() {
    // Rebuild when bundled assets change so include_bytes! consumers stay fresh.
    let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets");
    println!("cargo:rerun-if-changed={}", assets.display());
    for name in [
        "orbit_logo.ico",
        "orbit_logo.png",
        "orbit_logo_light.ico",
        "orbit_logo_light.png",
        "orbit_logo_dark.ico",
        "orbit_logo_dark.png",
    ] {
        let path = assets.join(name);
        if !path.exists() {
            println!("cargo:warning=asset missing: assets/{name}");
        }
        println!("cargo:rerun-if-changed={}", path.display());
    }
    println!("cargo:rerun-if-changed=locales");
    println!("cargo:rerun-if-changed=build.rs");
}
