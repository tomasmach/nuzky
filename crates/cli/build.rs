//! Loads the Vulkan loader on first use, for the reason crates/analysis/build.rs gives.
fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_ENV").is_ok_and(|env| env == "msvc") {
        println!("cargo::rustc-link-arg=/DELAYLOAD:vulkan-1.dll");
        println!("cargo::rustc-link-arg=delayimp.lib");
    }
}
