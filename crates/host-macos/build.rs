#[cfg(target_os = "macos")]
fn main() {
    // Cargo does not propagate a dependency build script's linker arguments to
    // this crate's test executable. Add the Swift runtime search paths here so
    // `cargo test -p nfidb-host-macos` exercises the real ScreenCaptureKit code.
    println!("cargo:rustc-link-arg=-Wl,-rpath,@executable_path/../Frameworks");
    println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
}

#[cfg(not(target_os = "macos"))]
fn main() {}
