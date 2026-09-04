fn main() -> std::io::Result<()> {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut resource = winresource::WindowsResource::new();
        resource
            .set_icon("../../assets/logo-icon.ico")
            .set("FileDescription", "Haucet CLI")
            .set("ProductName", "Haucet")
            .set("OriginalFilename", "haucet.exe");
        resource.compile()?;
    }
    Ok(())
}
