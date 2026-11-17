//! Compiles the Slint UI with the touch-sized `material` style.
//!
//! The app's own design tokens live in `ui/theme.slint`.

fn main() {
    // The shared kit (`crates/pleochroma-ui-kit`) is imported as `@pleochroma-kit`.
    let config = slint_build::CompilerConfiguration::new()
        .with_style("material".to_string())
        .with_library_paths(pleochroma_ui_kit::library_paths());
    slint_build::compile_with_config("ui/app.slint", config).expect("the Slint UI compiles");
    println!("cargo:rerun-if-changed=ui/");
}
