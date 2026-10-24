//! whisper.cpp links the Vulkan loader, which Windows has only with a graphics driver. Loading it on first use lets
//! Nuzky start without one. ggml then starts Vulkan inside a `catch (...)`, which whisper-rs-sys builds without
//! `/EHsc`, so it also catches the exception for the library that did not load. ggml leaves Vulkan out and speech runs
//! on the processor.
fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_ENV").is_ok_and(|env| env == "msvc") {
        println!("cargo::rustc-link-arg=/DELAYLOAD:vulkan-1.dll");
        println!("cargo::rustc-link-arg=delayimp.lib");
    }
}
