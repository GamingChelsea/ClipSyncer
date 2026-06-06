fn main() {
    let config = slint_build::CompilerConfiguration::new();
    slint_build::compile_with_config("ui/app.slint", config).expect("Slint Compile Fehler");
}
