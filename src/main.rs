//! Binary shell: starts an automatic Tracy capture in debug builds, then the engine.

#[cfg(all(feature = "tracy", debug_assertions))]
#[global_allocator]
static GLOBAL: tracy_client::ProfiledAllocator<std::alloc::System> =
    tracy_client::ProfiledAllocator::new(std::alloc::System, 0);

/// Seconds of capture per run; `0` disables automatic capture.
#[cfg(all(feature = "tracy", debug_assertions))]
const CAPTURE_SECONDS_ENV: &str = "TRACY_CAPTURE_SECONDS";

/// Starts `tracy-capture` writing to `reports/tracy/run-<unix>.tracy`.
///
/// Debug builds are profiled by default (`tracy` is a default feature). The
/// capture tool connects to the client over loopback; if it is missing, the run
/// continues and only a warning is printed.
#[cfg(all(feature = "tracy", debug_assertions))]
fn start_capture() {
    let seconds = std::env::var(CAPTURE_SECONDS_ENV)
        .ok()
        .and_then(|value| value.trim().parse::<u32>().ok())
        .unwrap_or(30);
    if seconds == 0 {
        return;
    }
    let Some(capture) = find_capture() else {
        eprintln!("tracy: tracy-capture not found; set TRACY_CAPTURE or put it on PATH");
        return;
    };
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    let directory = std::path::Path::new("reports/tracy");
    if let Err(error) = std::fs::create_dir_all(directory) {
        eprintln!("tracy: cannot create {}: {error}", directory.display());
        return;
    }
    let output = directory.join(format!("run-{stamp}.tracy"));
    match std::process::Command::new(capture)
        .arg("-o")
        .arg(&output)
        .arg("-s")
        .arg(seconds.to_string())
        .spawn()
    {
        Ok(child) => eprintln!(
            "tracy: capturing {seconds}s to {} (pid {})",
            output.display(),
            child.id()
        ),
        Err(error) => eprintln!("tracy: failed to start capture: {error}"),
    }
}

/// Locates `tracy-capture`: `TRACY_CAPTURE`, then `PATH`, then `%TEMP%\tracy`.
#[cfg(all(feature = "tracy", debug_assertions))]
fn find_capture() -> Option<std::path::PathBuf> {
    let exe = if cfg!(windows) {
        "tracy-capture.exe"
    } else {
        "tracy-capture"
    };
    if let Ok(path) = std::env::var("TRACY_CAPTURE") {
        let path = std::path::PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    if let Ok(path) = std::env::var("PATH") {
        for directory in std::env::split_paths(&path) {
            let candidate = directory.join(exe);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    if let Ok(temp) = std::env::var("TEMP") {
        let candidate = std::path::Path::new(&temp).join("tracy").join(exe);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn main() -> Result<(), engine::RunError> {
    #[cfg(all(feature = "tracy", debug_assertions))]
    start_capture();
    engine::run()
}
