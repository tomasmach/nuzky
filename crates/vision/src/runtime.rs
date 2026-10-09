//! ONNX Runtime is a shared library loaded the first time a model runs, not linked. Microsoft's
//! official build picks its CPU kernels at run time, so it works on any x86-64 processor, and when
//! it cannot load only covers are unavailable: the app itself never depends on it to start.
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{Result, anyhow};

/// Keep in step with VERSION in scripts/fetch-onnxruntime.mjs, which puts this build beside the app.
pub const RUNTIME_VERSION: &str = "1.28.3";

#[cfg(target_os = "linux")]
const LIBRARY: &str = "libonnxruntime.so.1";
#[cfg(target_os = "macos")]
const LIBRARY: &str = "libonnxruntime.1.dylib";
#[cfg(windows)]
const LIBRARY: &str = "onnxruntime.dll";

static LOADED: OnceLock<Result<PathBuf, String>> = OnceLock::new();

/// Loads ONNX Runtime once; every later call answers from that first attempt. Fails with
/// `VISION_UNAVAILABLE` saying why, so nothing downloads models that could not run.
pub fn require() -> Result<()> {
    match LOADED.get_or_init(load) {
        Ok(_) => Ok(()),
        Err(reason) => Err(anyhow!(
            "VISION_UNAVAILABLE: {reason}. Cover frames and subject masks cannot run; editing and export still work."
        )),
    }
}

fn load() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().and_then(|exe| exe.canonicalize()).map_err(|e| format!("finding Nuzky: {e}"))?;
    let override_path = std::env::var_os("NUZKY_ONNXRUNTIME").map(PathBuf::from);
    let candidates = candidates(&exe, override_path, cfg!(debug_assertions).then(dev_library).flatten());
    let Some(path) = candidates.iter().find(|path| path.is_file()) else {
        let hint = if cfg!(debug_assertions) { "run `node scripts/fetch-onnxruntime.mjs`" } else { "reinstall Nuzky" };
        return Err(format!("ONNX Runtime {RUNTIME_VERSION} is missing ({}); {hint}", candidates[0].display()));
    };
    let builder = ort::init_from(path)
        .map_err(|e| format!("ONNX Runtime could not start on this computer: {:#}", anyhow::Error::new(e)))?;
    // Microsoft's Windows build traces usage for telemetry unless told not to.
    builder.with_telemetry(false).commit();
    Ok(path.clone())
}

/// Where the library may be, in order: an explicit override (then only there), the installed
/// app's own copy beside the executable, and for debug builds the copy the fetch script keeps.
fn candidates(exe: &Path, override_path: Option<PathBuf>, dev: Option<PathBuf>) -> Vec<PathBuf> {
    if let Some(path) = override_path {
        return vec![path];
    }
    let dir = exe.parent().unwrap_or(Path::new("."));
    let bundled = if cfg!(target_os = "linux") {
        // Tauri's resource directory, named after productName in src-tauri/tauri.conf.json.
        dir.join("../lib/Nuzky").join(LIBRARY)
    } else if cfg!(target_os = "macos") {
        dir.join("../Frameworks").join(LIBRARY)
    } else {
        dir.join(LIBRARY)
    };
    std::iter::once(bundled).chain(dev).collect()
}

/// The fetch script's copy, in the build dependencies directory of the machine that compiled this.
fn dev_library() -> Option<PathBuf> {
    let deps = match option_env!("NUZKY_DEPS") {
        Some(deps) => PathBuf::from(deps),
        None if cfg!(windows) => PathBuf::from(option_env!("LOCALAPPDATA")?).join("Nuzky/build-deps"),
        None => PathBuf::from(option_env!("HOME")?).join(".cache/nuzky/deps"),
    };
    Some(deps.join(format!("onnxruntime-{RUNTIME_VERSION}")).join(LIBRARY))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_installed_copy_comes_before_the_build_copy_and_an_override_stands_alone() {
        let dev = PathBuf::from("/deps/onnxruntime/lib");
        let exe = Path::new("/opt/Nuzky/bin/nuzky-app");
        let found = candidates(exe, None, Some(dev.clone()));
        assert_eq!(found.len(), 2);
        assert!(found[0].starts_with("/opt/Nuzky/bin") && found[0].ends_with(LIBRARY), "{found:?}");
        assert_eq!(found[1], dev);
        let only = PathBuf::from("/elsewhere/libonnxruntime");
        assert_eq!(candidates(exe, Some(only.clone()), Some(dev)), [only]);
    }

    /// A one-node model run through the loaded library: proves on every system that it loads and
    /// runs, which nothing else checks without media and models.
    #[test]
    fn the_runtime_loads_and_runs_a_model() {
        require().unwrap();
        // ONNX `Relu` of a float tensor of length 4.
        const RELU: &[u8] = include_bytes!("../tests/data/relu.onnx");
        let mut session = ort::session::Session::builder().unwrap().commit_from_memory(RELU).unwrap();
        let input = ort::value::TensorRef::from_array_view(([4usize], &[-1.0f32, 0.0, 2.5, -0.5][..])).unwrap();
        let outputs = session.run(ort::inputs![input]).unwrap();
        let (_, values) = outputs[0].try_extract_tensor::<f32>().unwrap();
        assert_eq!(values, [0.0, 0.0, 2.5, 0.0]);
    }
}
