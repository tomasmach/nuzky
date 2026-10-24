//! ggml reads `GGML_VK_DISABLE_F16` with the C runtime's getenv, which on Windows keeps its own copy of the
//! environment.

#[test]
fn ggml_sees_vulkan_fp32() {
    // SAFETY: the only test in this binary, so no other thread touches the environment.
    unsafe { nuzky_analysis::keep_vulkan_fp32() };
    unsafe extern "C" {
        fn getenv(name: *const std::ffi::c_char) -> *const std::ffi::c_char;
    }
    let seen = unsafe { getenv(c"GGML_VK_DISABLE_F16".as_ptr()) };
    let seen = (!seen.is_null()).then(|| unsafe { std::ffi::CStr::from_ptr(seen) }.to_str().unwrap().to_owned());
    assert_eq!(seen, std::env::var("GGML_VK_DISABLE_F16").ok());
    assert!(seen.is_some());
}
