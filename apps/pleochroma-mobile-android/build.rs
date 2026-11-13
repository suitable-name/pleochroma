//! Compiles the Slint UI with the touch-sized `material` style.
//!
//! The app's own design tokens live in `ui/theme.slint`.

fn main() {
    let config = slint_build::CompilerConfiguration::new().with_style("material".to_string());
    slint_build::compile_with_config("ui/app.slint", config).expect("the Slint UI compiles");
    println!("cargo:rerun-if-changed=ui/");
}
