//! Windows resources: the app icon and version info.

fn main() {
    println!("cargo:rerun-if-changed=assets/app.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/app.ico")
        .set("ProductName", "Простор")
        .set("FileDescription", "Простор — программы и место на диске")
        .set("CompanyName", "Простор")
        .set("LegalCopyright", "Простор");
    // GPUI embeds its own manifest (DPI awareness), a second one would clash
    if let Err(e) = res.compile() {
        println!("cargo:warning=resources were not embedded: {e}");
    }
}
