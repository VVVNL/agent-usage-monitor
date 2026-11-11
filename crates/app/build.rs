fn main() {
    println!("cargo:rerun-if-changed=ui");
    // The UI is only built where the shell exists (Windows, macOS); the pure logic crates build
    // anywhere.
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if os == "windows" || os == "macos" {
        slint_build::compile("ui/app.slint").expect("compile ui/app.slint");
    }
}
