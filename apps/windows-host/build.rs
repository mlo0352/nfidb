fn main() {
    #[cfg(target_os = "windows")]
    {
        let mut resource = winresource::WindowsResource::new();
        resource.set("ProductName", "NFiDB");
        resource.set("FileDescription", "No Frills iPad Drawing Bridge");
        resource.set("LegalCopyright", "NFiDB contributors");
        if let Err(error) = resource.compile() {
            println!("cargo:warning=Windows resource metadata was not embedded: {error}");
        }
    }
}
