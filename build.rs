#[cfg(windows)]
fn main() {
    let mut resource = winresource::WindowsResource::new();
    resource
        .set_icon("packaging/windows/BeyondSlides.ico")
        .set("FileDescription", "BeyondSlides lecture reader")
        .set("ProductName", "BeyondSlides")
        .set("LegalCopyright", "Copyright BeyondSlides contributors");
    resource
        .compile()
        .expect("could not embed the BeyondSlides Windows resources");
}

#[cfg(not(windows))]
fn main() {}
