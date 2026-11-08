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

    // 4. Check ~/.mumei/toolchains/llvm-*/bin/clang — the layout
    // `mumei setup` actually installs (versioned dirs, not toolchains/bin).
    if let Some(home) = dirs::home_dir() {
        #[cfg(windows)]
        let clang_name = "clang.exe";
        #[cfg(not(windows))]
        let clang_name = "clang";
        let toolchains = home.join(".mumei/toolchains");
        if let Ok(entries) = std::fs::read_dir(&toolchains) {
            let mut candidates: Vec<PathBuf> = entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .map(|n| n.starts_with("llvm-"))
                        .unwrap_or(false)
                })
                .map(|p| p.join("bin").join(clang_name))
                .filter(|p| p.exists())
                .collect();
            candidates.sort();
            if let Some(clang) = candidates.into_iter().next() {
                return Ok(clang);
            }
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

/// Locate the libz3 directory inside `~/.mumei/toolchains/z3-*/` (`bin` on
/// Windows-style archives, `lib` as a fallback) provisioned by `mumei setup`.
#[cfg(not(windows))]
fn find_bundled_z3_lib_dir() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    let toolchains = home.join(".mumei/toolchains");
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&toolchains)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("z3-"))
                .unwrap_or(false)
        })
        .collect();
    dirs.sort();
    for dir in dirs {
        for sub in ["bin", "lib"] {
            let candidate = dir.join(sub);
            if candidate.join("libz3.so").exists() || candidate.join("libz3.dylib").exists() {
                return Some(candidate);
            }
        }
    }
    None
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
    run_linker(&mut link_command(
        &linker,
        inputs,
        output_path,
        runtime_lib_path,
    ))
}

fn link_command(
    linker: &Path,
    inputs: &[PathBuf],
    output_path: &Path,
    runtime_lib_path: Option<&Path>,
) -> Command {
    let mut cmd = Command::new(linker);
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
        cmd.arg("-ldl");
        // `mumei setup` installs libz3 under ~/.mumei/toolchains/z3-*/bin,
        // which is not on the default linker path — add -L/-rpath so the
        // unconditional -lz3 resolves for setup-provisioned toolchains.
        if let Some(dir) = find_bundled_z3_lib_dir() {
            let dir = dir.to_string_lossy().to_string();
            cmd.arg(format!("-L{}", dir));
            cmd.arg(format!("-Wl,-rpath,{}", dir));
        }
        cmd.arg("-lz3");
    }

    cmd
}

fn run_linker(cmd: &mut Command) -> Result<(), String> {
    let output = cmd.output().map_err(|error| {
        format!(
            "Failed to execute linker '{}': {}",
            cmd.get_program().to_string_lossy(),
            error
        )
    })?;

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

/// Observe the selected driver and its linker before executing the same command.
/// No raw tool output, environment or path-bearing arguments enter the record.
pub fn link_recorded(
    inputs: &[PathBuf],
    output: &Path,
) -> Result<mumei_core::native_bundle::LinkBuild, String> {
    use mumei_core::native_bundle::{sha256, LinkBuild};
    let fail = || "Unsupported native bundle linker observation".to_owned();
    let linker = find_c_linker().map_err(|_| fail())?;
    let driver = linker
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(fail)?;
    if !["cc", "gcc", "clang", "clang-17"].contains(&driver) {
        return Err(fail());
    }
    let observe = |flag: &str| -> Result<String, String> {
        let result = Command::new(&linker)
            .arg(flag)
            .output()
            .map_err(|_| fail())?;
        if !result.status.success() {
            return Err(fail());
        }
        String::from_utf8(result.stdout).map_err(|_| fail())
    };
    let version = regex::Regex::new(r"\b[0-9]+\.[0-9]+(?:\.[0-9]+)?\b").unwrap();
    let driver_banner = observe("--version")?;
    let driver_version = version
        .find(driver_banner.lines().next().unwrap_or(""))
        .ok_or_else(fail)?
        .as_str()
        .to_owned();
    let driver_target = observe("-dumpmachine")?.trim().to_owned();
    if !driver_target.starts_with("x86_64-")
        || !driver_target.contains("linux")
        || !driver_target
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
    {
        return Err(fail());
    }
    let mut command = link_command(&linker, inputs, output, None);
    let args: Vec<_> = command
        .get_args()
        .map(|arg| arg.as_encoded_bytes().to_vec())
        .collect();
    let arguments_sha256 = sha256(&serde_json::to_vec(&args).map_err(|_| fail())?);
    let mut probe = link_command(&linker, inputs, output, None);
    let result = probe.arg("-Wl,--version").output().map_err(|_| fail())?;
    if !result.status.success() {
        return Err(fail());
    }
    let banner = String::from_utf8(result.stdout).map_err(|_| fail())?;
    let line = banner
        .lines()
        .find(|line| line.starts_with("GNU ld ") || line.starts_with("LLD "))
        .ok_or_else(fail)?;
    let linker_version = version.find(line).ok_or_else(fail)?.as_str().to_owned();
    let linker_family = if line.starts_with("GNU ld ") {
        "gnu-ld"
    } else {
        "lld"
    };
    run_linker(&mut command).map_err(|_| "Native bundle linking failed".to_owned())?;
    Ok(LinkBuild {
        driver: driver.to_owned(),
        driver_version,
        driver_target,
        linker: linker_family.to_owned(),
        linker_version,
        arguments_sha256,
    })
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
