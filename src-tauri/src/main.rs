#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // Vulkan FP16 moves Whisper word times by up to 330 ms; FP32 is as fast. ggml reads this when
    // its backend starts, so it is set here, before any other thread exists.
    if std::env::var_os("GGML_VK_DISABLE_F16").is_none() {
        unsafe { std::env::set_var("GGML_VK_DISABLE_F16", "1") };
    }
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
