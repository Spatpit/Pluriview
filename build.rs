#[cfg(windows)]
fn main() {
    let mut res = winres::WindowsResource::new();
    res.set_icon("assets/icon.ico");
    // Set additional metadata
    res.set("ProductName", "Pluriview");
    res.set("FileDescription", "Live window preview application");
    res.compile().unwrap();
}

#[cfg(not(windows))]
fn main() {}
