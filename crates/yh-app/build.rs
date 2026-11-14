// Where the app finds sherpa-onnx's dylibs (yh-voice's Cargo.toml says why they're shared):
// the bundle's Frameworks folder, where scripts/bundle.sh puts them (and takes the second path
// out), then where its build unpacked them, for runs out of target/.
fn main() {
    println!("cargo:rustc-link-arg=-Wl,-rpath,@executable_path/../Frameworks");
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    // OUT_DIR is <target>/<profile>/build/<crate>/out.
    let target = out.ancestors().nth(4).unwrap();
    let libs = target.join("sherpa-onnx-prebuilt/sherpa-onnx-v2.0.1-osx-arm64-shared-lib/lib");
    println!("cargo:rustc-link-arg=-Wl,-rpath,{}", libs.display());
}
