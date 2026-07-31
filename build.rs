#[cfg(target_os = "windows")]
fn main() {
    use winres::{VersionInfo, WindowsResource};

    println!("cargo:rerun-if-changed=assets/promplet.ico");

    // Cargo.toml is the only place the version is written; the Windows
    // four-part resource version carries the alpha number in its last slot,
    // so "0.1.0-alpha.6" becomes 0.1.0.6.
    let version = std::env::var("CARGO_PKG_VERSION").expect("cargo provides the package version");
    let (base, alpha) = version.split_once("-alpha.").unwrap_or((&version, "0"));
    let numbers: Vec<u64> = base
        .split('.')
        .chain([alpha])
        .map(|part| part.parse().expect("version parts are numeric"))
        .collect();
    let &[major, minor, patch, build] = numbers.as_slice() else {
        panic!("the package version must have three numeric parts");
    };
    let packed = (major << 48) | (minor << 32) | (patch << 16) | build;

    let mut resources = WindowsResource::new();
    resources
        .set_icon("assets/promplet.ico")
        .set("FileDescription", "Promplet — clipboard-free prompt strip")
        .set("ProductName", "Promplet")
        .set("ProductVersion", &version)
        .set("FileVersion", &format!("{major}.{minor}.{patch}.{build}"))
        .set("OriginalFilename", "promplet.exe")
        .set("LegalCopyright", "Copyright © 2026 TylerSystems")
        .set_version_info(VersionInfo::FILEVERSION, packed)
        .set_version_info(VersionInfo::PRODUCTVERSION, packed);

    resources
        .compile()
        .expect("failed to compile Windows icon and version resources");
}

#[cfg(not(target_os = "windows"))]
fn main() {}
