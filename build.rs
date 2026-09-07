use sha2::{Digest, Sha256};

const UBOL_ARCHIVE_PATH: &str = "assets/third_party/ubol/uBOLite_2026.714.1952.edge.zip";

fn configure_local_performance() {
    println!("cargo:rustc-check-cfg=cfg(pluriview_performance)");
    println!("cargo:rerun-if-env-changed=PLURIVIEW_LOCAL_PERFORMANCE");
    if std::env::var_os("PLURIVIEW_LOCAL_PERFORMANCE").as_deref() == Some(std::ffi::OsStr::new("1"))
    {
        assert!(
            cfg!(windows) && std::path::Path::new("src/app/performance.rs").is_file(),
            "Local performance testing requires Windows and the workstation-only harness"
        );
        println!("cargo:rerun-if-changed=src/app/performance.rs");
        println!("cargo:rustc-cfg=pluriview_performance");
    }
}

fn expose_ubol_fingerprint() {
    println!("cargo:rerun-if-changed={UBOL_ARCHIVE_PATH}");
    let archive =
        std::fs::read(UBOL_ARCHIVE_PATH).expect("read embedded uBlock Origin Lite archive");
    println!(
        "cargo:rustc-env=PLURIVIEW_UBOL_SHA256={:x}",
        Sha256::digest(archive)
    );
}

#[cfg(windows)]
fn main() {
    configure_local_performance();
    expose_ubol_fingerprint();
    println!("cargo:rerun-if-changed=assets/icon.ico");
    let mut res = winres::WindowsResource::new();
    res.set_icon("assets/icon.ico");
    // Set additional metadata
    res.set("ProductName", "Pluriview");
    res.set("FileDescription", "Pluriview");
    res.compile().unwrap();
}

#[cfg(not(windows))]
fn main() {
    configure_local_performance();
    expose_ubol_fingerprint();
}
