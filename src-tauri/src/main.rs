#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // SAFETY: no other thread exists yet.
    unsafe { nuzky_analysis::keep_vulkan_fp32() };
    #[cfg(all(target_os = "macos", debug_assertions))]
    nuzky_app::test_bridge::take_token();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|arg| arg == "mcp") {
        let cache = dirs::cache_dir().unwrap_or_else(std::env::temp_dir).join("nuzky");
        if let Err(error) = nuzky_mcp::bridge::run_args(&args[1..], cache) {
            eprintln!("{error:#}");
            std::process::exit(1);
        }
        return;
    }
    nuzky_app::run();
}
