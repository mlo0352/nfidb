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
}
