fn main() {
    tauri_build::build();
    // Integration tests using Tauri's MockRuntime also link comctl32 v6 APIs.
    // Tauri embeds the application's manifest, but not Cargo integration-test manifests.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rustc-link-arg-tests=/MANIFEST:EMBED");
        println!(r#"cargo:rustc-link-arg-tests=/MANIFESTDEPENDENCY:type='win32' name='Microsoft.Windows.Common-Controls' version='6.0.0.0' processorArchitecture='*' publicKeyToken='6595b64144ccf1df' language='*'"#);
    }
}
