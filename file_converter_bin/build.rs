fn main() {
    let config = slint_build::CompilerConfiguration::new().with_style("fluent".to_string());
    slint_build::compile_with_config("ui/appwindow.slint", config).unwrap();

    if std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() == "windows" {
        let mut res = winres::WindowsResource::new();
        res.set_icon("../icon.ico");
        if let Err(e) = res.compile() {
            eprintln!("Failed to compile Windows resource icon: {}", e);
        }
    }
}
