//! # Manifest モジュール
//!
//! `mumei.toml` の解析と設定値の提供を行う。
//!
//! ## 対応セクション
//! - `[package]`: プロジェクトメタデータ（name, version, authors, description）
//! - `[dependencies]`: パッケージ依存（path / git / version）
//! - `[build]`: ビルド設定（targets, verify, max_unroll）
//! - `[proof]`: 検証設定（cache, timeout_ms）
//! - `[effects]`: エフェクト境界設定（allowed, denied）
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
// =============================================================================
// mumei.toml 構造体定義
// =============================================================================
/// mumei.toml のトップレベル構造
#[derive(Debug, Clone, Deserialize)]
pub struct Manifest {
    pub package: Package,
    #[serde(default)]
    pub dependencies: HashMap<String, Dependency>,
    #[serde(default)]
    pub build: BuildConfig,
    #[serde(default)]
    pub proof: ProofConfig,
    #[serde(default)]
    pub effects: EffectsConfig,
}
/// [package] セクション
#[derive(Debug, Clone, Deserialize)]
pub struct Package {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub authors: Vec<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub repository: Option<String>,
}
/// 依存パッケージの指定方法
/// - 文字列: バージョンのみ（例: "0.1.0"）
/// - テーブル: 詳細指定（path / git / rev / tag）
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Dependency {
    /// バージョン文字列のみ: `math = "0.1.0"`
    Version(String),
    /// 詳細指定: `math = { path = "./libs/math" }` or `math = { git = "...", tag = "v1.0.0" }`
    Detailed(DependencyDetail),
}
#[derive(Debug, Clone, Deserialize)]
pub struct DependencyDetail {
    pub version: Option<String>,
    pub path: Option<String>,
    pub git: Option<String>,
    pub rev: Option<String>,
    pub tag: Option<String>,
    pub branch: Option<String>,
}
/// [build] セクション
#[derive(Debug, Clone, Deserialize)]
pub struct BuildConfig {
    /// トランスパイル対象言語（デフォルト: ["rust", "go", "typescript"]）
    #[serde(default = "default_targets")]
    pub targets: Vec<String>,
    /// Z3 検証を実行するか（デフォルト: true）
    #[serde(default = "default_true")]
    pub verify: bool,
    /// BMC 展開深度（デフォルト: 3）
    #[serde(default = "default_max_unroll")]
    pub max_unroll: usize,
}
impl Default for BuildConfig {
    fn default() -> Self {
        Self {
            targets: default_targets(),
            verify: true,
            max_unroll: 3,
        }
    }
}
/// [proof] セクション
#[derive(Debug, Clone, Deserialize)]
pub struct ProofConfig {
    /// 検証キャッシュを使用するか（デフォルト: true）
    #[serde(default = "default_true")]
    pub cache: bool,
    /// Z3 ソルバのタイムアウト（ミリ秒、デフォルト: 10000）
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
}
impl Default for ProofConfig {
    fn default() -> Self {
        Self {
            cache: true,
            timeout_ms: 10000,
        }
    }
}
/// [effects] セクション — AIエージェントセッションの許可エフェクト設定
#[derive(Debug, Clone, Default, Deserialize)]
pub struct EffectsConfig {
    /// デフォルトで許可されるエフェクト (例: ["Log", "FileRead"])
    /// 空の場合は全エフェクトが許可される（制限なし）
    #[serde(default)]
    pub allowed: Vec<String>,
    /// 拒否されるエフェクト（allowed より優先される）
    #[serde(default)]
    pub denied: Vec<String>,
}
// =============================================================================
// デフォルト値ヘルパー
// =============================================================================
fn default_targets() -> Vec<String> {
    vec![
        "rust".to_string(),
        "go".to_string(),
        "typescript".to_string(),
    ]
}
fn default_true() -> bool {
    true
}
fn default_max_unroll() -> usize {
    3
}
fn default_timeout() -> u64 {
    10000
}
// =============================================================================
// マニフェスト読み込み
// =============================================================================
/// 指定パスの mumei.toml を読み込んでパースする
pub fn load(path: &Path) -> Result<Manifest, ManifestError> {
    let content = fs::read_to_string(path).map_err(|e| ManifestError::Io(path.to_path_buf(), e))?;
    let manifest: Manifest = toml::from_str(&content)
        .map_err(|e| ManifestError::Parse(path.to_path_buf(), e.to_string()))?;
    Ok(manifest)
}
/// カレントディレクトリから上方向に mumei.toml を探索して読み込む
/// 見つかった場合は (mumei.toml のあるディレクトリ, Manifest) を返す
pub fn find_and_load() -> Option<(PathBuf, Manifest)> {
    let mut dir = std::env::current_dir().ok()?;
    loop {
        let manifest_path = dir.join("mumei.toml");
        if manifest_path.exists() {
            match load(&manifest_path) {
                Ok(manifest) => return Some((dir, manifest)),
                Err(e) => {
                    eprintln!("  ⚠️  Failed to parse {}: {}", manifest_path.display(), e);
                    return None;
                }
            }
        }
        if !dir.pop() {
            break;
        }
    }
    None
}
/// ~/.mumei/ のパスを返す
pub fn mumei_home() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".mumei")
}
// =============================================================================
// Dependency ヘルパー
// =============================================================================
impl Dependency {
    /// パス依存かどうか
    pub fn as_path(&self) -> Option<&str> {
        match self {
            Dependency::Detailed(d) => d.path.as_deref(),
            _ => None,
        }
    }
    /// Git 依存かどうか — (url, tag, rev, branch) を返す
    #[allow(clippy::type_complexity)]
    pub fn as_git(&self) -> Option<(&str, Option<&str>, Option<&str>, Option<&str>)> {
        match self {
            Dependency::Detailed(d) => d
                .git
                .as_deref()
                .map(|url| (url, d.tag.as_deref(), d.rev.as_deref(), d.branch.as_deref())),
            _ => None,
        }
    }
    /// バージョン文字列を取得
    pub fn version(&self) -> Option<&str> {
        match self {
            Dependency::Version(v) => Some(v.as_str()),
            Dependency::Detailed(d) => d.version.as_deref(),
        }
    }
}
// =============================================================================
// エラー型
// =============================================================================
#[derive(Debug)]
pub enum ManifestError {
    Io(PathBuf, std::io::Error),
    Parse(PathBuf, String),
}
impl std::fmt::Display for ManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ManifestError::Io(path, e) => write!(f, "Cannot read '{}': {}", path.display(), e),
            ManifestError::Parse(path, e) => {
                write!(f, "Parse error in '{}': {}", path.display(), e)
            }
        }
    }
}
impl std::error::Error for ManifestError {}
