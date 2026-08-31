//! # Setup モジュール
//!
//! `mumei setup` コマンドの実装。
//! Z3 と LLVM 18 のプリビルドバイナリをダウンロードし、
//! `~/.mumei/toolchains/` に配置する。
//!
//! ## ディレクトリ構造
//! ```text
//! ~/.mumei/
//! ├── toolchains/
//! │   ├── z3-{version}/
//! │   │   ├── bin/z3, bin/libz3.{so,dylib,a}
//! │   │   └── include/z3.h
//! │   └── llvm-{version}/
//! │       ├── bin/llc
//! │       ├── lib/
//! │       └── include/
//! └── env                  # source ~/.mumei/env で環境変数設定
//! ```
use mumei_core::manifest;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as Cmd;
// =============================================================================
// バージョン定数
// =============================================================================
const LLVM_VERSION: &str = "18.1.8";

/// Upstream Z3 prebuilt archives are named after the image they were built on
/// (`z3-{version}-{arch}-{osx-<ver>|glibc-<ver>}`), and the suffix changes
/// between releases, so every field is pinned per release. The `min_glibc`
/// values are the highest `GLIBC_` symbol version `bin/libz3.so` actually
/// imports (`objdump -T`), which can be lower than the name of the archive.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Z3Build {
    version: &'static str,
    macos_suffix: &'static str,
    linux_x64_suffix: &'static str,
    linux_x64_min_glibc: (u32, u32),
    linux_arm64_suffix: &'static str,
    linux_arm64_min_glibc: (u32, u32),
}

/// Z3 5.1.0: monadic regex solver, string/quantified-array soundness fixes.
const Z3_BUILD: Z3Build = Z3Build {
    version: "5.1.0",
    macos_suffix: "osx-13.3",
    linux_x64_suffix: "glibc-2.39",
    linux_x64_min_glibc: (2, 38),
    linux_arm64_suffix: "glibc-2.38",
    linux_arm64_min_glibc: (2, 38),
};

/// Last release with prebuilt archives for glibc 2.34/2.35 hosts
/// (Ubuntu 22.04, RHEL 9, Debian 12).
const Z3_LEGACY_BUILD: Z3Build = Z3Build {
    version: "4.14.1",
    macos_suffix: "osx-13.7.4",
    linux_x64_suffix: "glibc-2.35",
    linux_x64_min_glibc: (2, 34),
    linux_arm64_suffix: "glibc-2.34",
    linux_arm64_min_glibc: (2, 34),
};

// =============================================================================
// エラー型
// =============================================================================

#[derive(Debug)]
pub enum SetupError {
    UnsupportedPlatform(String),
    Io(String),
    Command(String),
}

impl std::fmt::Display for SetupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SetupError::UnsupportedPlatform(msg) => write!(f, "{}", msg),
            SetupError::Io(msg) => write!(f, "{}", msg),
            SetupError::Command(msg) => write!(f, "{}", msg),
        }
    }
}

impl std::error::Error for SetupError {}
// =============================================================================
// プラットフォーム検出
// =============================================================================
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Os {
    MacOS,
    Linux,
}
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Arch {
    X86_64,
    Aarch64,
}
#[derive(Debug, Clone, Copy)]
pub struct Platform {
    pub os: Os,
    pub arch: Arch,
}
impl Platform {
    pub fn detect() -> Result<Self, SetupError> {
        let os = match std::env::consts::OS {
            "macos" => Os::MacOS,
            "linux" => Os::Linux,
            other => {
                return Err(SetupError::UnsupportedPlatform(format!(
                    "Unsupported OS: {}. mumei setup supports macOS and Linux.",
                    other
                )))
            }
        };
        let arch = match std::env::consts::ARCH {
            "x86_64" => Arch::X86_64,
            "aarch64" => Arch::Aarch64,
            other => {
                return Err(SetupError::UnsupportedPlatform(format!(
                    "Unsupported architecture: {}. mumei setup supports x86_64 and aarch64.",
                    other
                )))
            }
        };
        Ok(Platform { os, arch })
    }
    fn z3_archive_name(&self, build: &Z3Build) -> String {
        let (arch, suffix) = match (self.os, self.arch) {
            (Os::MacOS, Arch::Aarch64) => ("arm64", build.macos_suffix),
            (Os::MacOS, Arch::X86_64) => ("x64", build.macos_suffix),
            (Os::Linux, Arch::X86_64) => ("x64", build.linux_x64_suffix),
            (Os::Linux, Arch::Aarch64) => ("arm64", build.linux_arm64_suffix),
        };
        format!("z3-{}-{}-{}", build.version, arch, suffix)
    }
    fn z3_download_url(&self, build: &Z3Build) -> String {
        format!(
            "https://github.com/Z3Prover/z3/releases/download/z3-{}/{}.zip",
            build.version,
            self.z3_archive_name(build)
        )
    }
    /// Minimum glibc the prebuilt Linux archive of `build` was linked against.
    fn required_glibc(&self, build: &Z3Build) -> Option<(u32, u32)> {
        match (self.os, self.arch) {
            (Os::MacOS, _) => None,
            (Os::Linux, Arch::X86_64) => Some(build.linux_x64_min_glibc),
            (Os::Linux, Arch::Aarch64) => Some(build.linux_arm64_min_glibc),
        }
    }
    fn llvm_archive_name(&self) -> String {
        match (self.os, self.arch) {
            (Os::MacOS, Arch::Aarch64) => {
                format!("clang+llvm-{}-arm64-apple-darwin24.2.0", LLVM_VERSION)
            }
            (Os::MacOS, Arch::X86_64) => format!("clang+llvm-{}-x86_64-apple-darwin", LLVM_VERSION),
            (Os::Linux, Arch::X86_64) => {
                format!("clang+llvm-{}-x86_64-linux-gnu-ubuntu-18.04", LLVM_VERSION)
            }
            (Os::Linux, Arch::Aarch64) => format!("clang+llvm-{}-aarch64-linux-gnu", LLVM_VERSION),
        }
    }
    fn llvm_download_url(&self) -> String {
        let archive = self.llvm_archive_name();
        format!(
            "https://github.com/llvm/llvm-project/releases/download/llvmorg-{}/{}.tar.xz",
            LLVM_VERSION, archive
        )
    }
}
// =============================================================================
// メイン処理
// =============================================================================

/// `mumei setup` のエントリポイント
pub fn run(force: bool) {
    println!("🔧 Mumei Setup: configuring toolchain...");
    println!();

    // プラットフォーム検出
    let platform = match Platform::detect() {
        Ok(p) => {
            let os_str = match p.os {
                Os::MacOS => "macOS",
                Os::Linux => "Linux",
            };
            let arch_str = match p.arch {
                Arch::X86_64 => "x86_64",
                Arch::Aarch64 => "aarch64",
            };
            println!("  📋 Platform: {} {}", os_str, arch_str);
            p
        }
        Err(e) => {
            eprintln!("  ❌ {}", e);
            std::process::exit(1);
        }
    };

    let mumei_home = manifest::mumei_home();
    let toolchains_dir = mumei_home.join("toolchains");

    if let Err(e) = fs::create_dir_all(&toolchains_dir) {
        eprintln!("  ❌ Failed to create {}: {}", toolchains_dir.display(), e);
        std::process::exit(1);
    }

    // --- Z3 ---
    let z3_build = match select_z3_build(&platform, detect_host_glibc()) {
        Ok(build) => Some(build),
        Err(e) => {
            eprintln!("  ❌ Z3: {}", e);
            None
        }
    };
    let z3_dir = z3_build.and_then(|build| {
        let dir = toolchains_dir.join(format!("z3-{}", build.version));
        match install_z3(&platform, &build, &toolchains_dir, &dir, force) {
            Ok(()) => Some(dir),
            Err(e) => {
                eprintln!("  ❌ Z3 install failed: {}", e);
                eprintln!(
                    "     Fallback: install from system package manager (e.g. brew/apt) and re-run."
                );
                None
            }
        }
    });

    // --- LLVM ---
    let llvm_dir = toolchains_dir.join(format!("llvm-{}", LLVM_VERSION));
    if let Err(e) = install_llvm(&platform, &toolchains_dir, &llvm_dir, force) {
        eprintln!("  ❌ LLVM install failed: {}", e);
        eprintln!("     Fallback: install from system package manager (e.g. brew/apt) and re-run.");
    }

    // --- env スクリプト生成 ---
    if let Err(e) = generate_env_script(&mumei_home, z3_dir.as_deref(), &llvm_dir) {
        eprintln!("  ⚠️  Failed to generate env script: {}", e);
    }

    // --- 簡易検証 ---
    verify_installation(z3_dir.as_deref(), &llvm_dir);

    println!();
    println!("🎉 Setup complete!");
    println!("   Run: source ~/.mumei/env");
}

/// Read the host glibc version from `ldd --version`.
fn detect_host_glibc() -> Option<(u32, u32)> {
    if std::env::consts::OS != "linux" {
        return None;
    }
    let out = Cmd::new("ldd").arg("--version").output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    parse_glibc_version(&text)
}

fn parse_glibc_version(ldd_output: &str) -> Option<(u32, u32)> {
    let first = ldd_output.lines().next()?;
    let version = first.split_whitespace().last()?;
    let (major, minor) = version.split_once('.')?;
    let minor = minor
        .split(|c: char| !c.is_ascii_digit())
        .next()
        .unwrap_or(minor);
    Some((major.parse().ok()?, minor.parse().ok()?))
}

/// Prefer the pinned Z3 release, falling back to the legacy build when the
/// host glibc predates the prebuilt archive's build image. Hosts older than
/// the legacy build, and non-glibc hosts, have no usable prebuilt archive.
fn select_z3_build(
    platform: &Platform,
    host_glibc: Option<(u32, u32)>,
) -> Result<Z3Build, SetupError> {
    let Some(required) = platform.required_glibc(&Z3_BUILD) else {
        return Ok(Z3_BUILD);
    };
    let Some(host) = host_glibc else {
        println!("  ⚠️  Could not determine the host glibc version from `ldd --version`");
        println!(
            "     Trying Z3 {} (oldest supported glibc). On musl hosts, install z3 from your",
            Z3_LEGACY_BUILD.version
        );
        println!("     package manager instead — upstream publishes no musl archive.");
        return Ok(Z3_LEGACY_BUILD);
    };
    if host >= required {
        return Ok(Z3_BUILD);
    }
    let legacy_required = platform
        .required_glibc(&Z3_LEGACY_BUILD)
        .unwrap_or(Z3_LEGACY_BUILD.linux_x64_min_glibc);
    if host < legacy_required {
        return Err(SetupError::UnsupportedPlatform(format!(
            "host glibc {}.{} is older than every prebuilt Z3 archive (Z3 {} needs {}.{}, \
             Z3 {} needs {}.{}); install z3 from your package manager or build it from source",
            host.0,
            host.1,
            Z3_BUILD.version,
            required.0,
            required.1,
            Z3_LEGACY_BUILD.version,
            legacy_required.0,
            legacy_required.1
        )));
    }
    println!(
        "  ⚠️  Z3 {} prebuilt binaries require glibc {}.{}, host has {}.{}",
        Z3_BUILD.version, required.0, required.1, host.0, host.1
    );
    println!(
        "     Falling back to Z3 {}. Build Z3 {} from source for the newer solver.",
        Z3_LEGACY_BUILD.version, Z3_BUILD.version
    );
    Ok(Z3_LEGACY_BUILD)
}

fn install_z3(
    platform: &Platform,
    build: &Z3Build,
    toolchains_dir: &Path,
    z3_dir: &Path,
    force: bool,
) -> Result<(), SetupError> {
    if z3_dir.exists() {
        if !force && z3_dir.join("bin").join("z3").exists() {
            println!("  ✅ Z3 {}: already installed", build.version);
            return Ok(());
        }
        fs::remove_dir_all(z3_dir)
            .map_err(|e| SetupError::Io(format!("Failed to remove {}: {}", z3_dir.display(), e)))?;
    }

    println!("  📦 Downloading Z3 {}...", build.version);
    println!("     URL: {}", platform.z3_download_url(build));

    let archive_path =
        download_with_curl(&platform.z3_download_url(build), toolchains_dir, "z3.zip")?;
    extract_zip(&archive_path, toolchains_dir)?;

    let extracted = toolchains_dir.join(platform.z3_archive_name(build));
    if !extracted.exists() {
        return Err(SetupError::Io(format!(
            "Expected extracted directory not found: {}",
            extracted.display()
        )));
    }

    fs::rename(&extracted, z3_dir).map_err(|e| {
        SetupError::Io(format!(
            "Failed to move {} -> {}: {}",
            extracted.display(),
            z3_dir.display(),
            e
        ))
    })?;

    let _ = fs::remove_file(&archive_path);
    println!(
        "  ✅ Z3 {}: installed to {}",
        build.version,
        z3_dir.display()
    );
    Ok(())
}

fn install_llvm(
    platform: &Platform,
    toolchains_dir: &Path,
    llvm_dir: &Path,
    force: bool,
) -> Result<(), SetupError> {
    if llvm_dir.exists() {
        if !force {
            println!("  ✅ LLVM {}: already installed", LLVM_VERSION);
            return Ok(());
        }
        fs::remove_dir_all(llvm_dir).map_err(|e| {
            SetupError::Io(format!("Failed to remove {}: {}", llvm_dir.display(), e))
        })?;
    }

    println!("  📦 Downloading LLVM {}...", LLVM_VERSION);
    println!("     URL: {}", platform.llvm_download_url());
    println!("     ⚠️  This is a large download (~hundreds of MB)");

    let archive_path =
        download_with_curl(&platform.llvm_download_url(), toolchains_dir, "llvm.tar.xz")?;
    extract_tar_xz(&archive_path, toolchains_dir)?;

    let extracted = toolchains_dir.join(platform.llvm_archive_name());
    if !extracted.exists() {
        return Err(SetupError::Io(format!(
            "Expected extracted directory not found: {}",
            extracted.display()
        )));
    }

    fs::rename(&extracted, llvm_dir).map_err(|e| {
        SetupError::Io(format!(
            "Failed to move {} -> {}: {}",
            extracted.display(),
            llvm_dir.display(),
            e
        ))
    })?;

    let _ = fs::remove_file(&archive_path);
    println!(
        "  ✅ LLVM {}: installed to {}",
        LLVM_VERSION,
        llvm_dir.display()
    );
    Ok(())
}

/// `z3_dir` is `None` when no prebuilt archive is usable on this host; the
/// generated script then leaves Z3 to the system installation instead of
/// pointing at a missing toolchain.
fn generate_env_script(
    mumei_home: &Path,
    z3_dir: Option<&Path>,
    llvm_dir: &Path,
) -> Result<(), SetupError> {
    fs::create_dir_all(mumei_home)
        .map_err(|e| SetupError::Io(format!("Failed to create {}: {}", mumei_home.display(), e)))?;

    let env_path = mumei_home.join("env");
    let z3 = z3_dir.map(|d| d.display().to_string());
    let llvm = llvm_dir.display().to_string();

    let mut lines = vec![
        "#!/bin/sh".to_string(),
        "# Mumei toolchain environment — generated by `mumei setup`".to_string(),
        "# Usage: source ~/.mumei/env".to_string(),
        String::new(),
    ];
    // Upstream Z3 archives ship libz3.{so,dylib,a} in `bin`, not `lib`, and
    // z3-sys links libz3 dynamically, so the loader needs `bin` too.
    match &z3 {
        Some(z3) => {
            lines.push("# Z3".to_string());
            lines.push(format!("export Z3_SYS_Z3_HEADER=\"{}/include/z3.h\"", z3));
            lines.push(format!("export Z3_SYS_Z3_LIB_DIR=\"{}/bin\"", z3));
            lines.push(format!("export CPATH=\"{}/include:$CPATH\"", z3));
            lines.push(format!("export LIBRARY_PATH=\"{}/bin:$LIBRARY_PATH\"", z3));
            if cfg!(target_os = "macos") {
                lines.push(format!(
                    "export DYLD_FALLBACK_LIBRARY_PATH=\"{}/bin:$DYLD_FALLBACK_LIBRARY_PATH\"",
                    z3
                ));
            } else {
                lines.push(format!(
                    "export LD_LIBRARY_PATH=\"{}/bin:$LD_LIBRARY_PATH\"",
                    z3
                ));
            }
        }
        None => {
            lines.push("# Z3: no bundled toolchain on this host — using the system install".into());
        }
    }
    lines.push(String::new());
    lines.push("# LLVM".to_string());
    lines.push(format!("export LLVM_SYS_170_PREFIX=\"{}\"", llvm));
    lines.push(format!("export PATH=\"{}/bin:$PATH\"", llvm));
    lines.push(match &z3 {
        Some(z3) => format!("export LDFLAGS=\"-L{}/lib -L{}/bin $LDFLAGS\"", llvm, z3),
        None => format!("export LDFLAGS=\"-L{}/lib $LDFLAGS\"", llvm),
    });
    lines.push(match &z3 {
        Some(z3) => format!(
            "export CPPFLAGS=\"-I{}/include -I{}/include $CPPFLAGS\"",
            llvm, z3
        ),
        None => format!("export CPPFLAGS=\"-I{}/include $CPPFLAGS\"", llvm),
    });
    lines.push(String::new());

    let content = lines.join("\n");

    fs::write(&env_path, content)
        .map_err(|e| SetupError::Io(format!("Failed to write {}: {}", env_path.display(), e)))?;

    println!("  ✅ Generated {}", env_path.display());
    Ok(())
}

fn verify_installation(z3_dir: Option<&Path>, llvm_dir: &Path) {
    println!();
    println!("🔍 Verifying toolchain...");

    match z3_dir.map(|d| d.join("bin").join("z3")) {
        Some(z3_bin) if z3_bin.exists() => report_version("Z3 (toolchain)", &z3_bin),
        Some(z3_bin) => {
            println!("  ⚠️  Z3 (toolchain): not found at {}", z3_bin.display());
        }
        None => println!("  ⚠️  Z3: not installed — install z3 from your package manager"),
    }

    // llc は LLVM アーカイブに入っている想定
    let llc_bin = llvm_dir.join("bin").join("llc");
    if llc_bin.exists() {
        report_version("LLVM (toolchain)", &llc_bin);
    } else {
        println!("  ⚠️  LLVM (toolchain): not found at {}", llc_bin.display());
    }
}

/// A binary that spawns but exits non-zero or prints nothing is broken (e.g. a
/// prebuilt archive whose shared library requirements the host cannot satisfy),
/// so it must not be reported as a working install.
fn report_version(label: &str, bin: &Path) {
    match Cmd::new(bin).arg("--version").output() {
        Ok(o) => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            let version = stdout.lines().next().unwrap_or("").trim();
            if o.status.success() && !version.is_empty() {
                println!("  ✅ {}: {}", label, version);
            } else {
                let stderr = String::from_utf8_lossy(&o.stderr);
                println!(
                    "  ⚠️  {}: {} exists but `--version` failed ({}){}",
                    label,
                    bin.display(),
                    o.status,
                    stderr
                        .lines()
                        .next()
                        .map(|l| format!(": {}", l.trim()))
                        .unwrap_or_default()
                );
            }
        }
        Err(e) => println!("  ⚠️  {} exists but failed to run: {}", label, e),
    }
}

// =============================================================================
// Download/extract helpers (external tools)
// =============================================================================

fn download_with_curl(url: &str, dest_dir: &Path, filename: &str) -> Result<PathBuf, SetupError> {
    let dest = dest_dir.join(filename);
    let status = Cmd::new("curl")
        .args(["-fSL", "--progress-bar", "-o"])
        .arg(&dest)
        .arg(url)
        .status()
        .map_err(|e| SetupError::Command(format!("Failed to run curl: {}", e)))?;

    if !status.success() {
        return Err(SetupError::Command(format!(
            "curl failed with exit code: {:?}",
            status.code()
        )));
    }

    Ok(dest)
}

fn extract_zip(archive: &Path, dest_dir: &Path) -> Result<(), SetupError> {
    let status = Cmd::new("unzip")
        .args(["-q", "-o"])
        .arg(archive)
        .arg("-d")
        .arg(dest_dir)
        .status()
        .map_err(|e| SetupError::Command(format!("Failed to run unzip: {}", e)))?;

    if !status.success() {
        return Err(SetupError::Command(format!(
            "unzip failed with exit code: {:?}",
            status.code()
        )));
    }
    Ok(())
}

fn extract_tar_xz(archive: &Path, dest_dir: &Path) -> Result<(), SetupError> {
    let status = Cmd::new("tar")
        .args(["xf"])
        .arg(archive)
        .arg("-C")
        .arg(dest_dir)
        .status()
        .map_err(|e| SetupError::Command(format!("Failed to run tar: {}", e)))?;

    if !status.success() {
        return Err(SetupError::Command(format!(
            "tar failed with exit code: {:?}",
            status.code()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINUX_X64: Platform = Platform {
        os: Os::Linux,
        arch: Arch::X86_64,
    };
    const LINUX_ARM64: Platform = Platform {
        os: Os::Linux,
        arch: Arch::Aarch64,
    };
    const MACOS_ARM64: Platform = Platform {
        os: Os::MacOS,
        arch: Arch::Aarch64,
    };

    #[test]
    fn archive_names_match_upstream_release_assets() {
        assert_eq!(
            LINUX_X64.z3_archive_name(&Z3_BUILD),
            "z3-5.1.0-x64-glibc-2.39"
        );
        assert_eq!(
            LINUX_ARM64.z3_archive_name(&Z3_BUILD),
            "z3-5.1.0-arm64-glibc-2.38"
        );
        assert_eq!(
            MACOS_ARM64.z3_archive_name(&Z3_BUILD),
            "z3-5.1.0-arm64-osx-13.3"
        );
        assert_eq!(
            LINUX_X64.z3_archive_name(&Z3_LEGACY_BUILD),
            "z3-4.14.1-x64-glibc-2.35"
        );
        assert_eq!(
            LINUX_ARM64.z3_archive_name(&Z3_LEGACY_BUILD),
            "z3-4.14.1-arm64-glibc-2.34"
        );
    }

    #[test]
    fn download_url_points_at_the_release_tag() {
        assert_eq!(
            LINUX_X64.z3_download_url(&Z3_BUILD),
            "https://github.com/Z3Prover/z3/releases/download/z3-5.1.0/z3-5.1.0-x64-glibc-2.39.zip"
        );
    }

    #[test]
    fn glibc_version_is_parsed_from_ldd_output() {
        assert_eq!(
            parse_glibc_version("ldd (Ubuntu GLIBC 2.35-0ubuntu3.8) 2.35\nCopyright\n"),
            Some((2, 35))
        );
        assert_eq!(parse_glibc_version("ldd (GNU libc) 2.39\n"), Some((2, 39)));
        assert_eq!(parse_glibc_version("musl libc (x86_64)\n"), None);
    }

    #[test]
    fn old_glibc_hosts_fall_back_to_the_legacy_build() {
        assert_eq!(
            select_z3_build(&LINUX_X64, Some((2, 35))).unwrap(),
            Z3_LEGACY_BUILD
        );
        assert_eq!(
            select_z3_build(&LINUX_ARM64, Some((2, 34))).unwrap(),
            Z3_LEGACY_BUILD
        );
        assert_eq!(
            select_z3_build(&LINUX_X64, Some((2, 38))).unwrap(),
            Z3_BUILD
        );
        assert_eq!(
            select_z3_build(&LINUX_ARM64, Some((2, 38))).unwrap(),
            Z3_BUILD
        );
        assert_eq!(
            select_z3_build(&MACOS_ARM64, Some((2, 17))).unwrap(),
            Z3_BUILD
        );
    }

    #[test]
    fn undetectable_libc_tries_the_oldest_supported_build() {
        assert_eq!(select_z3_build(&LINUX_X64, None).unwrap(), Z3_LEGACY_BUILD);
    }

    #[test]
    fn generated_env_points_at_the_directory_holding_libz3() {
        let home = std::env::temp_dir().join(format!("mumei-env-test-{}", std::process::id()));
        let z3 = home.join("toolchains").join("z3-5.1.0");
        let llvm = home.join("toolchains").join("llvm-18.1.8");
        generate_env_script(&home, Some(&z3), &llvm).unwrap();
        let env = fs::read_to_string(home.join("env")).unwrap();
        let z3 = z3.display().to_string();
        assert!(env.contains(&format!("Z3_SYS_Z3_LIB_DIR=\"{}/bin\"", z3)));
        assert!(env.contains(&format!("LIBRARY_PATH=\"{}/bin:", z3)));
        assert!(env.contains(&format!("-L{}/bin ", z3)));
        assert!(env.contains(&format!("Z3_SYS_Z3_HEADER=\"{}/include/z3.h\"", z3)));
        let loader = if cfg!(target_os = "macos") {
            "DYLD_FALLBACK_LIBRARY_PATH"
        } else {
            "LD_LIBRARY_PATH"
        };
        assert!(env.contains(&format!("{}=\"{}/bin:", loader, z3)));
        assert!(!env.contains(&format!("{}/lib", z3)));
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn env_without_a_bundled_z3_exports_no_z3_paths() {
        let home = std::env::temp_dir().join(format!("mumei-env-none-{}", std::process::id()));
        let llvm = home.join("toolchains").join("llvm-18.1.8");
        generate_env_script(&home, None, &llvm).unwrap();
        let env = fs::read_to_string(home.join("env")).unwrap();
        assert!(!env.contains("Z3_SYS_Z3_LIB_DIR"));
        assert!(!env.contains("Z3_SYS_Z3_HEADER"));
        assert!(env.contains(&format!("-L{}/lib $LDFLAGS", llvm.display())));
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn hosts_older_than_every_archive_are_rejected() {
        for host in [(2, 17), (2, 33)] {
            assert!(select_z3_build(&LINUX_X64, Some(host)).is_err());
            assert!(select_z3_build(&LINUX_ARM64, Some(host)).is_err());
        }
    }
}
