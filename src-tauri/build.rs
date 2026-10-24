use std::process::Command;

fn main() {
    let git = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .output()
            .ok()
            .filter(|out| out.status.success())
            .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_owned())
    };
    let commit = git(&["rev-parse", "--short=7", "HEAD"]).unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=NUZKY_COMMIT={commit}");
    let reference = git(&["rev-parse", "--symbolic-full-name", "HEAD"]);
    for name in [Some("HEAD"), reference.as_deref()].into_iter().flatten() {
        if let Some(path) = git(&["rev-parse", "--git-path", name]) {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    // whisper.cpp links the Vulkan loader, which Windows has only with a graphics driver. Loading it on first use
    // lets Nuzky start without one; crates/analysis/build.rs says why speech then runs on the processor.
    if std::env::var("CARGO_CFG_TARGET_ENV").is_ok_and(|env| env == "msvc") {
        println!("cargo:rustc-link-arg=/DELAYLOAD:vulkan-1.dll");
        println!("cargo:rustc-link-arg=delayimp.lib");
    }
    tauri_build::build()
}
