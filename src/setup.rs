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
//! │   │   ├── bin/z3
//! │   │   ├── lib/libz3.{so,dylib}
//! │   │   └── include/z3.h
//! │   └── llvm-{version}/
//! │       ├── bin/llc
//! │       ├── lib/
//! │       └── include/
//! └── env                  # source ~/.mumei/env で環境変数設定
//! ```
use crate::manifest;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as Cmd;
// =============================================================================
// バージョン定数
// =============================================================================
const Z3_VERSION: &str = "4.13.4";
const LLVM_VERSION: &str = "18.1.8";

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
    fn z3_archive_name(&self) -> String {
        match (self.os, self.arch) {
            (Os::MacOS, Arch::Aarch64) => format!("z3-{}-arm64-osx-13.7.1", Z3_VERSION),
            (Os::MacOS, Arch::X86_64) => format!("z3-{}-x64-osx-13.7.1", Z3_VERSION),
            (Os::Linux, Arch::X86_64) => format!("z3-{}-x64-glibc-2.35", Z3_VERSION),
            (Os::Linux, Arch::Aarch64) => format!("z3-{}-arm64-glibc-2.35", Z3_VERSION),
        }
    }
    fn z3_download_url(&self) -> String {
        let archive = self.z3_archive_name();
        format!(
            "https://github.com/Z3Prover/z3/releases/download/z3-{}/{}.zip",
            Z3_VERSION, archive
        )
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
    let z3_dir = toolchains_dir.join(format!("z3-{}", Z3_VERSION));
    if let Err(e) = install_z3(&platform, &toolchains_dir, &z3_dir, force) {
        eprintln!("  ❌ Z3 install failed: {}", e);
        eprintln!("     Fallback: install from system package manager (e.g. brew/apt) and re-run.");
    }

    // --- LLVM ---
    let llvm_dir = toolchains_dir.join(format!("llvm-{}", LLVM_VERSION));
    if let Err(e) = install_llvm(&platform, &toolchains_dir, &llvm_dir, force) {
        eprintln!("  ❌ LLVM install failed: {}", e);
        eprintln!("     Fallback: install from system package manager (e.g. brew/apt) and re-run.");
    }

    // --- env スクリプト生成 ---
    if let Err(e) = generate_env_script(&mumei_home, &z3_dir, &llvm_dir) {
        eprintln!("  ⚠️  Failed to generate env script: {}", e);
    }

    // --- 簡易検証 ---
    verify_installation(&z3_dir, &llvm_dir);

    println!();
    println!("🎉 Setup complete!");
    println!("   Run: source ~/.mumei/env");
}

fn install_z3(
    platform: &Platform,
    toolchains_dir: &Path,
    z3_dir: &Path,
    force: bool,
) -> Result<(), SetupError> {
    if z3_dir.exists() {
        if !force {
            println!("  ✅ Z3 {}: already installed", Z3_VERSION);
            return Ok(());
        }
        fs::remove_dir_all(z3_dir)
            .map_err(|e| SetupError::Io(format!("Failed to remove {}: {}", z3_dir.display(), e)))?;
    }

    println!("  📦 Downloading Z3 {}...", Z3_VERSION);
    println!("     URL: {}", platform.z3_download_url());

    let archive_path = download_with_curl(&platform.z3_download_url(), toolchains_dir, "z3.zip")?;
    extract_zip(&archive_path, toolchains_dir)?;

    let extracted = toolchains_dir.join(platform.z3_archive_name());
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
    println!("  ✅ Z3 {}: installed to {}", Z3_VERSION, z3_dir.display());
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

fn generate_env_script(
    mumei_home: &Path,
    z3_dir: &Path,
    llvm_dir: &Path,
) -> Result<(), SetupError> {
    fs::create_dir_all(mumei_home)
        .map_err(|e| SetupError::Io(format!("Failed to create {}: {}", mumei_home.display(), e)))?;

    let env_path = mumei_home.join("env");
    let z3 = z3_dir.display().to_string();
    let llvm = llvm_dir.display().to_string();

    let mut lines = Vec::new();
    lines.push("#!/bin/sh".to_string());
    lines.push("# Mumei toolchain environment — generated by `mumei setup`".to_string());
    lines.push("# Usage: source ~/.mumei/env".to_string());
    lines.push(String::new());
    lines.push("# Z3".to_string());
    lines.push(format!("export Z3_SYS_Z3_HEADER=\"{}/include/z3.h\"", z3));
    lines.push(format!("export Z3_SYS_Z3_LIB_DIR=\"{}/lib\"", z3));
    lines.push(format!("export CPATH=\"{}/include:$CPATH\"", z3));
    lines.push(format!("export LIBRARY_PATH=\"{}/lib:$LIBRARY_PATH\"", z3));
    lines.push(String::new());
    lines.push("# LLVM".to_string());
    lines.push(format!("export LLVM_SYS_170_PREFIX=\"{}\"", llvm));
    lines.push(format!("export PATH=\"{}/bin:$PATH\"", llvm));
    lines.push(format!(
        "export LDFLAGS=\"-L{}/lib -L{}/lib $LDFLAGS\"",
        llvm, z3
    ));
    lines.push(format!(
        "export CPPFLAGS=\"-I{}/include -I{}/include $CPPFLAGS\"",
        llvm, z3
    ));
    lines.push(String::new());

    let content = lines.join("\n");

    fs::write(&env_path, content)
        .map_err(|e| SetupError::Io(format!("Failed to write {}: {}", env_path.display(), e)))?;

    println!("  ✅ Generated {}", env_path.display());
    Ok(())
}

fn verify_installation(z3_dir: &Path, llvm_dir: &Path) {
    println!();
    println!("🔍 Verifying toolchain...");

    let z3_bin = z3_dir.join("bin").join("z3");
    if z3_bin.exists() {
        let out = Cmd::new(&z3_bin).arg("--version").output();
        match out {
            Ok(o) => {
                let s = String::from_utf8_lossy(&o.stdout);
                println!("  ✅ Z3 (toolchain): {}", s.trim());
            }
            Err(e) => println!("  ⚠️  Z3 (toolchain) exists but failed to run: {}", e),
        }
    } else {
        println!("  ⚠️  Z3 (toolchain): not found at {}", z3_bin.display());
    }

    // llc は LLVM アーカイブに入っている想定
    let llc_bin = llvm_dir.join("bin").join("llc");
    if llc_bin.exists() {
        let out = Cmd::new(&llc_bin).arg("--version").output();
        match out {
            Ok(o) => {
                let s = String::from_utf8_lossy(&o.stdout);
                let first = s.lines().next().unwrap_or("");
                println!("  ✅ LLVM (toolchain): {}", first.trim());
            }
            Err(e) => println!("  ⚠️  LLVM (toolchain) exists but failed to run: {}", e),
        }
    } else {
        println!("  ⚠️  LLVM (toolchain): not found at {}", llc_bin.display());
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
