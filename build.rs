use sha2::{Digest, Sha256};

const UBOL_ARCHIVE_PATH: &str = "assets/third_party/ubol/uBOLite_2026.714.1952.edge.zip";

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
    expose_ubol_fingerprint();
    let mut res = winres::WindowsResource::new();
    res.set_icon("assets/icon.ico");
    // Set additional metadata
    res.set("ProductName", "Pluriview");
    res.set("FileDescription", "Live window preview application");
    res.compile().unwrap();
}

#[cfg(not(windows))]
fn main() {
    expose_ubol_fingerprint();
}
