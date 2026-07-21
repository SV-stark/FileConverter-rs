fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() == "windows" {
        let mut res = winres::WindowsResource::new();
        res.set_icon("../icon.ico");
        if let Err(e) = res.compile() {
            eprintln!("Failed to compile Windows resource icon: {}", e);
        }
    }
}
