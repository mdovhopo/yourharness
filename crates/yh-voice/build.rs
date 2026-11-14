// sherpa-onnx's dylibs (see Cargo.toml) where its build unpacked them, for this crate's tests and
// examples. The app's build.rs does the same, plus the bundle's Frameworks folder.
fn main() {
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    // OUT_DIR is <target>/<profile>/build/<crate>/out.
    let target = out.ancestors().nth(4).unwrap();
    let libs = target.join("sherpa-onnx-prebuilt/sherpa-onnx-v2.0.1-osx-arm64-shared-lib/lib");
    println!("cargo:rustc-link-arg=-Wl,-rpath,{}", libs.display());
}
