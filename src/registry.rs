//! # Registry モジュール
//!
//! ローカルパッケージレジストリ (`~/.mumei/registry.json`) の管理。
//! `mumei publish` で公開されたパッケージを名前＋バージョンで検索可能にする。
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
/// レジストリ全体
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Registry {
    /// パッケージ名 → パッケージメタデータ
    pub packages: HashMap<String, PackageEntry>,
}
/// 1つのパッケージの全バージョン情報
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageEntry {
    /// バージョン → バージョン詳細
    pub versions: HashMap<String, VersionEntry>,
    /// 最新バージョン
    pub latest: String,
}
/// 1つのバージョンの詳細
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionEntry {
    /// パッケージディレクトリの絶対パス
    pub path: String,
    /// 公開日時
    pub published_at: String,
    /// 含まれる atom 数
    pub atom_count: usize,
    /// 検証済みかどうか
    pub verified: bool,
}
/// レジストリファイルのパスを返す
pub fn registry_path() -> PathBuf {
    crate::manifest::mumei_home().join("registry.json")
}
/// レジストリを読み込む。存在しない場合は空のレジストリを返す。
pub fn load() -> Registry {
    let path = registry_path();
    fs::read_to_string(&path)
        .ok()
        .and_then(|content| serde_json::from_str(&content).ok())
        .unwrap_or_default()
}
/// レジストリを保存する。
pub fn save(registry: &Registry) -> Result<(), String> {
    let path = registry_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create {}: {}", parent.display(), e))?;
    }
    let json = serde_json::to_string_pretty(registry)
        .map_err(|e| format!("Failed to serialize registry: {}", e))?;
    fs::write(&path, json).map_err(|e| format!("Failed to write {}: {}", path.display(), e))?;
    Ok(())
}
/// パッケージ名とバージョン（省略時は latest）でパスを解決する。
/// 見つからなければ None を返す。
pub fn resolve(name: &str, version: Option<&str>) -> Option<PathBuf> {
    let registry = load();
    let entry = registry.packages.get(name)?;
    let ver = version.unwrap_or(&entry.latest);
    let ver_entry = entry.versions.get(ver)?;
    let p = PathBuf::from(&ver_entry.path);
    if p.exists() {
        Some(p)
    } else {
        None
    }
}
/// パッケージを登録する。同じ name+version が既にあれば上書き。
pub fn register(
    name: &str,
    version: &str,
    pkg_path: &Path,
    atom_count: usize,
    verified: bool,
) -> Result<(), String> {
    let mut registry = load();
    let now = chrono_lite_now();
    let ver_entry = VersionEntry {
        path: pkg_path.to_string_lossy().to_string(),
        published_at: now,
        atom_count,
        verified,
    };
    let pkg = registry
        .packages
        .entry(name.to_string())
        .or_insert_with(|| PackageEntry {
            versions: HashMap::new(),
            latest: version.to_string(),
        });
    pkg.versions.insert(version.to_string(), ver_entry);
    pkg.latest = version.to_string();
    save(&registry)
}
/// 簡易タイムスタンプ（外部クレート不要）
fn chrono_lite_now() -> String {
    use std::time::SystemTime;
    match SystemTime::now().duration_since(SystemTime::UNIX_EPOCH) {
        Ok(d) => format!("unix:{}", d.as_secs()),
        Err(_) => "unknown".to_string(),
    }
}
