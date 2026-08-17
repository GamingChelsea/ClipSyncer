fn main() {
    let config = slint_build::CompilerConfiguration::new();
    slint_build::compile_with_config("ui/app.slint", config).expect("Slint Compile Fehler");

    #[cfg(windows)]
    {
        let mut res = winres::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        res.compile().expect("Failed to compile Windows resources");
    }
}
