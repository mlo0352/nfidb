fn main() {
    #[cfg(target_os = "windows")]
    {
        let icon = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("assets")
            .join("nfidb.ico");
        println!("cargo:rerun-if-changed={}", icon.display());
        let mut resource = winresource::WindowsResource::new();
        resource.set("ProductName", "NFiDB");
        resource.set("FileDescription", "No Frills iPad Drawing Bridge");
        resource.set("LegalCopyright", "NFiDB contributors");
        resource.set_icon(icon.to_string_lossy().as_ref());
        if let Err(error) = resource.compile() {
            println!("cargo:warning=Windows resource metadata was not embedded: {error}");
        }
    }

    #[cfg(target_os = "macos")]
    configure_macos_swift_runtime();
}

#[cfg(target_os = "macos")]
fn configure_macos_swift_runtime() {
    // ScreenCaptureKit's small Swift bridge references the concurrency runtime.
    // macOS 13+ supplies it in the system Swift runtime. Keep an app-relative
    // fallback for a future signed bundle that deliberately embeds Swift.
    println!("cargo:rustc-link-arg=-Wl,-rpath,@executable_path/../Frameworks");
    println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
}
