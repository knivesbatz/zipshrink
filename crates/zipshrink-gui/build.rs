fn main() {
    println!("cargo:rerun-if-changed=../../assets/zipshrink.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../../assets/zipshrink.ico")
            .set("FileDescription", "ZipShrink desktop app")
            .set("ProductName", "ZipShrink")
            .set("OriginalFilename", "zipshrink-gui.exe")
            .set("LegalCopyright", "MIT License, ZipShrink contributors");
        if let Err(e) = res.compile() {
            println!("cargo:warning=could not embed Windows resources: {e}");
        }
    }
}
