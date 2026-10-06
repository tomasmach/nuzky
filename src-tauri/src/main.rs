#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // Vulkan FP16 moves Whisper word times by up to 330 ms; FP32 is as fast. ggml reads this when
    // its backend starts, so it is set here, before any other thread exists.
    if std::env::var_os("GGML_VK_DISABLE_F16").is_none() {
        unsafe { std::env::set_var("GGML_VK_DISABLE_F16", "1") };
    }
    capopen_app::run();
}
