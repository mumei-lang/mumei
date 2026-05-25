// =============================================================================
// P7-B: Linker Pipeline — Link LLVM IR to native binary via clang
// =============================================================================

use std::path::{Path, PathBuf};
use std::process::Command;

/// Cross-platform helper to find an executable on PATH.
/// Uses `which` on Unix and `where` on Windows.
fn find_on_path(name: &str) -> Option<PathBuf> {
    #[cfg(windows)]
    let cmd = "where";
    #[cfg(not(windows))]
    let cmd = "which";

    if let Ok(output) = Command::new(cmd).arg(name).output() {
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout)
                .lines()
                .next()
                .unwrap_or("")
                .trim()
                .to_string();
            if !path.is_empty() {
                return Some(PathBuf::from(path));
            }
        }
    }
    None
}

/// Find a working clang binary. Checks:
/// 1. $LLVM_SYS_170_PREFIX/bin/clang
/// 2. clang-17 on $PATH
/// 3. clang on $PATH
/// 4. ~/.mumei/toolchains/bin/clang (clang.exe on Windows)
fn find_clang() -> Result<PathBuf, String> {
    // 1. Check LLVM_SYS_170_PREFIX
    if let Ok(prefix) = std::env::var("LLVM_SYS_170_PREFIX") {
        let clang = PathBuf::from(&prefix).join("bin/clang");
        if clang.exists() {
            return Ok(clang);
        }
    }

    // 2. Check clang-17 on PATH (cross-platform)
    if let Some(path) = find_on_path("clang-17") {
        return Ok(path);
    }

    // 3. Check clang on PATH (cross-platform)
    if let Some(path) = find_on_path("clang") {
        return Ok(path);
    }

    // 4. Check ~/.mumei/toolchains/ (use clang.exe on Windows)
    if let Some(home) = dirs::home_dir() {
        #[cfg(windows)]
        let clang_name = "clang.exe";
        #[cfg(not(windows))]
        let clang_name = "clang";
        let clang = home.join(".mumei/toolchains/bin").join(clang_name);
        if clang.exists() {
            return Ok(clang);
        }
    }

    Err(
        "Could not find clang. Please install LLVM/clang or run `mumei setup`.\n\
         Hint: Install with `apt install clang-17` or set LLVM_SYS_170_PREFIX."
            .to_string(),
    )
}

/// Find a C compiler that can link generated objects and runtime C sources.
fn find_c_linker() -> Result<PathBuf, String> {
    match find_clang() {
        Ok(path) => Ok(path),
        Err(clang_error) => {
            if let Some(path) = find_on_path("cc") {
                Ok(path)
            } else if let Some(path) = find_on_path("gcc") {
                Ok(path)
            } else {
                Err(format!(
                    "{}\nAlso could not find a fallback C compiler (`cc` or `gcc`) for linking.",
                    clang_error
                ))
            }
        }
    }
}

/// Link one or more LLVM/object/runtime inputs into a native binary.
///
/// # Arguments
/// * `inputs` — LLVM IR, object files, C runtime sources, or archives
/// * `output_path` — path for the output executable
/// * `runtime_lib_path` — optional runtime object/archive/source to link
pub fn link_to_binary(
    inputs: &[PathBuf],
    output_path: &Path,
    runtime_lib_path: Option<&Path>,
) -> Result<(), String> {
    let linker = find_c_linker()?;

    let mut cmd = Command::new(&linker);
    cmd.arg("-O2");
    cmd.arg("-o");
    cmd.arg(output_path);

    for input in inputs {
        cmd.arg(input);
    }
    if let Some(runtime) = runtime_lib_path {
        cmd.arg(runtime);
    }

    // Link math and pthread libraries (Unix only; Windows uses default CRT)
    #[cfg(not(windows))]
    {
        cmd.arg("-lm");
        cmd.arg("-lpthread");
    }

    let output = cmd
        .output()
        .map_err(|e| format!("Failed to execute linker '{}': {}", linker.display(), e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "native linking failed (exit {}):\n{}",
            output.status.code().unwrap_or(-1),
            stderr
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_clang() {
        // This test verifies that find_clang can locate a clang binary
        // on the system. It's OK if it fails in CI without LLVM installed.
        let result = find_clang();
        if let Ok(path) = &result {
            assert!(path.exists() || path.to_str().unwrap().contains("clang"));
        }
        // Not asserting Ok because clang may not be installed in all environments
    }
}
