fn main() {
    let config = slint_build::CompilerConfiguration::new().with_style("fluent".into());
    if let Err(e) = slint_build::compile_with_config("ui/main.slint", config) {
        eprintln!("slint compile error: {e}");
        std::process::exit(1);
    }
}
