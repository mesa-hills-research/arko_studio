// Windows reads the app's icon from icon resource 1 in the executable, for the
// window, the taskbar and Explorer, and shows `FileDescription` as the app's
// name in Task Manager. Other platforms get their icon from packaging
// (packaging/linux on Linux, an app bundle on macOS).
fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        winresource::WindowsResource::new()
            .set_icon("assets/icon.ico")
            .set("FileDescription", "Arko Studio")
            .set("ProductName", "Arko Studio")
            .set("LegalCopyright", "Copyright © 2026 Luke Erickson")
            .compile()
            .expect("failed to embed the Windows icon");
    }
}
