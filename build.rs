#[cfg(target_os = "windows")]
fn main() {
    use winres::{VersionInfo, WindowsResource};

    println!("cargo:rerun-if-changed=assets/promplet.ico");

    let mut resources = WindowsResource::new();
    resources
        .set_icon("assets/promplet.ico")
        .set("FileDescription", "Promplet — clipboard-free prompt strip")
        .set("ProductName", "Promplet")
        .set("ProductVersion", "0.1.0-alpha.1")
        .set("FileVersion", "0.1.0.1")
        .set("OriginalFilename", "promplet.exe")
        .set("LegalCopyright", "Copyright © 2026 TylerSystems")
        .set_version_info(VersionInfo::FILEVERSION, 0x0000_0001_0000_0001)
        .set_version_info(VersionInfo::PRODUCTVERSION, 0x0000_0001_0000_0001);

    resources
        .compile()
        .expect("failed to compile Windows icon and version resources");
}

#[cfg(not(target_os = "windows"))]
fn main() {}
