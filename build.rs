//! screencapturekit links a Swift bridge, so the binary needs the Swift runtime on its rpath.
//! A dependency's `rustc-link-arg` doesn't reach our binary, so we repeat it here.
//! `/usr/lib/swift` is the system runtime (in the dyld shared cache since macOS 12).

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
    }
}
