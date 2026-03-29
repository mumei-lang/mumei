// =============================================================================
// P7-B: Linker Pipeline — Link LLVM IR to native binary via clang
// =============================================================================

use std::path::{Path, PathBuf};
use std::process::Command;

/// Find a working clang binary. Checks:
/// 1. $LLVM_SYS_170_PREFIX/bin/clang
/// 2. clang-17 on $PATH
/// 3. clang on $PATH
/// 4. ~/.mumei/toolchains/bin/clang
fn find_clang() -> Result<PathBuf, String> {
    // 1. Check LLVM_SYS_170_PREFIX
    if let Ok(prefix) = std::env::var("LLVM_SYS_170_PREFIX") {
        let clang = PathBuf::from(&prefix).join("bin/clang");
        if clang.exists() {
            return Ok(clang);
        }
    }

    // 2. Check clang-17 on PATH
    if let Ok(output) = Command::new("which").arg("clang-17").output() {
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            return Ok(PathBuf::from(path));
        }
    }

    // 3. Check clang on PATH
    if let Ok(output) = Command::new("which").arg("clang").output() {
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            return Ok(PathBuf::from(path));
        }
    }

    // 4. Check ~/.mumei/toolchains/
    if let Some(home) = dirs::home_dir() {
        let clang = home.join(".mumei/toolchains/bin/clang");
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

/// Link one or more .ll files into a native binary using clang.
///
/// # Arguments
/// * `ll_files` — paths to LLVM IR files to compile and link
/// * `output_path` — path for the output executable
/// * `_runtime_lib_path` — optional path to a runtime library (reserved for future use)
pub fn link_to_binary(
    ll_files: &[PathBuf],
    output_path: &Path,
    _runtime_lib_path: Option<&Path>,
) -> Result<(), String> {
    let clang = find_clang()?;

    let mut cmd = Command::new(&clang);
    cmd.arg("-O2");
    cmd.arg("-o");
    cmd.arg(output_path);

    for ll in ll_files {
        cmd.arg(ll);
    }

    // Link math and pthread libraries
    cmd.arg("-lm");
    cmd.arg("-lpthread");

    let output = cmd
        .output()
        .map_err(|e| format!("Failed to execute clang: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "clang linking failed (exit {}):\n{}",
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
