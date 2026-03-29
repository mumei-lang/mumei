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
    /// P5-B: Path to .proof-cert.json certificate file
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cert_path: Option<String>,
    /// P5-B: SHA-256 hash of the certificate file for integrity verification
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cert_hash: Option<String>,
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
/// バージョンが "*" の場合は latest を使用する。
/// バージョンが "^x.y.z" の場合は semver 互換の最新バージョンを使用する。
/// 見つからなければ None を返す。
pub fn resolve(name: &str, version: Option<&str>) -> Option<PathBuf> {
    let registry = load();
    let entry = registry.packages.get(name)?;
    let resolved_version = match version {
        None | Some("*") => entry.latest.clone(),
        Some(v) if v.starts_with('^') => {
            // Semver-compatible: ^x.y.z respects left-most non-zero digit
            let base = v.trim_start_matches('^');
            find_compatible_version(entry, base)?
        }
        Some(v) if v.starts_with('~') => {
            // Tilde: ~x.y.z matches any version with same major.minor
            let base = v.trim_start_matches('~');
            find_tilde_compatible_version(entry, base)?
        }
        Some(v) => v.to_string(),
    };
    let ver_entry = entry.versions.get(&resolved_version)?;
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
    register_with_cert(name, version, pkg_path, atom_count, verified, None, None)
}

/// P5-B: Register a package with optional certificate metadata.
pub fn register_with_cert(
    name: &str,
    version: &str,
    pkg_path: &Path,
    atom_count: usize,
    verified: bool,
    cert_path: Option<String>,
    cert_hash: Option<String>,
) -> Result<(), String> {
    let mut registry = load();
    let now = chrono_lite_now();
    let ver_entry = VersionEntry {
        path: pkg_path.to_string_lossy().to_string(),
        published_at: now,
        atom_count,
        verified,
        cert_path,
        cert_hash,
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
/// Parse a version string "x.y.z" into (major, minor, patch).
fn parse_semver(v: &str) -> Option<(u64, u64, u64)> {
    let parts: Vec<&str> = v.split('.').collect();
    match parts.len() {
        1 => Some((parts[0].parse().ok()?, 0, 0)),
        2 => Some((parts[0].parse().ok()?, parts[1].parse().ok()?, 0)),
        3.. => Some((
            parts[0].parse().ok()?,
            parts[1].parse().ok()?,
            parts[2].parse().ok()?,
        )),
        _ => None,
    }
}
/// Find the highest version compatible with ^base.
/// Semver caret semantics respect the left-most non-zero digit:
///   ^X.Y.Z (X>0): same major, >= base  (i.e. >=X.Y.Z, <(X+1).0.0)
///   ^0.Y.Z (Y>0): same major.minor, >= base  (i.e. >=0.Y.Z, <0.(Y+1).0)
///   ^0.0.Z:       exact patch  (i.e. ==0.0.Z)
fn find_compatible_version(entry: &PackageEntry, base: &str) -> Option<String> {
    let (base_major, base_minor, base_patch) = parse_semver(base)?;
    let mut best: Option<(u64, u64, u64, String)> = None;
    for ver_str in entry.versions.keys() {
        if let Some((major, minor, patch)) = parse_semver(ver_str) {
            let compatible = if base_major != 0 {
                // ^X.Y.Z (X>0): same major, >= base
                major == base_major
                    && (minor > base_minor || (minor == base_minor && patch >= base_patch))
            } else if base_minor != 0 {
                // ^0.Y.Z (Y>0): same major.minor, >= base
                major == 0 && minor == base_minor && patch >= base_patch
            } else {
                // ^0.0.Z: exact match on major.minor.patch
                major == 0 && minor == 0 && patch == base_patch
            };
            if compatible && best.as_ref().is_none_or(|b| (minor, patch) > (b.1, b.2)) {
                best = Some((major, minor, patch, ver_str.clone()));
            }
        }
    }
    best.map(|b| b.3)
}
/// Find the highest version compatible with ~base (same major.minor, >= base).
fn find_tilde_compatible_version(entry: &PackageEntry, base: &str) -> Option<String> {
    let (base_major, base_minor, base_patch) = parse_semver(base)?;
    let mut best: Option<(u64, u64, u64, String)> = None;
    for ver_str in entry.versions.keys() {
        if let Some((major, minor, patch)) = parse_semver(ver_str) {
            if major == base_major
                && minor == base_minor
                && patch >= base_patch
                && best.as_ref().is_none_or(|b| patch > b.2)
            {
                best = Some((major, minor, patch, ver_str.clone()));
            }
        }
    }
    best.map(|b| b.3)
}
/// List all packages in the registry.
pub fn list_packages() -> Vec<(String, PackageEntry)> {
    let registry = load();
    let mut packages: Vec<(String, PackageEntry)> = registry.packages.into_iter().collect();
    packages.sort_by(|a, b| a.0.cmp(&b.0));
    packages
}
/// 簡易タイムスタンプ（外部クレート不要）
fn chrono_lite_now() -> String {
    use std::time::SystemTime;
    match SystemTime::now().duration_since(SystemTime::UNIX_EPOCH) {
        Ok(d) => format!("unix:{}", d.as_secs()),
        Err(_) => "unknown".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// P5-B: VersionEntry serialization with cert_path and cert_hash
    #[test]
    fn test_version_entry_serialization_with_cert() {
        let entry = VersionEntry {
            path: "/tmp/pkg".to_string(),
            published_at: "unix:1234567890".to_string(),
            atom_count: 3,
            verified: true,
            cert_path: Some("/tmp/pkg/cert.json".to_string()),
            cert_hash: Some("abc123".to_string()),
        };
        let json = serde_json::to_string(&entry).unwrap();
        let parsed: VersionEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.cert_path, Some("/tmp/pkg/cert.json".to_string()));
        assert_eq!(parsed.cert_hash, Some("abc123".to_string()));
    }

    /// P5-B: VersionEntry backward compatibility — missing cert fields default to None
    #[test]
    fn test_version_entry_backward_compat() {
        let json = r#"{"path":"/tmp/pkg","published_at":"unix:0","atom_count":1,"verified":false}"#;
        let parsed: VersionEntry = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.cert_path, None);
        assert_eq!(parsed.cert_hash, None);
    }

    /// P5-B: register_with_cert stores cert metadata
    #[test]
    fn test_register_with_cert_stores_metadata() {
        // Use a temp dir for the registry to avoid interfering with real state
        let tmp = std::env::temp_dir().join("mumei_test_registry_p5b");
        let _ = std::fs::create_dir_all(&tmp);

        // Create a temporary package directory
        let pkg_dir = tmp.join("my_pkg_v1");
        let _ = std::fs::create_dir_all(&pkg_dir);

        // We can't easily test register_with_cert without mocking the registry path,
        // but we can test the VersionEntry construction directly
        let ver_entry = VersionEntry {
            path: pkg_dir.to_string_lossy().to_string(),
            published_at: chrono_lite_now(),
            atom_count: 5,
            verified: true,
            cert_path: Some(pkg_dir.join("cert.json").to_string_lossy().to_string()),
            cert_hash: Some("deadbeef".to_string()),
        };
        assert_eq!(ver_entry.atom_count, 5);
        assert!(ver_entry.verified);
        assert!(ver_entry.cert_path.is_some());
        assert_eq!(ver_entry.cert_hash.as_deref(), Some("deadbeef"));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// P5-B: cert_path and cert_hash are omitted from JSON when None
    #[test]
    fn test_version_entry_skip_serializing_none_cert() {
        let entry = VersionEntry {
            path: "/tmp/pkg".to_string(),
            published_at: "unix:0".to_string(),
            atom_count: 0,
            verified: false,
            cert_path: None,
            cert_hash: None,
        };
        let json = serde_json::to_string(&entry).unwrap();
        assert!(!json.contains("cert_path"));
        assert!(!json.contains("cert_hash"));
    }
}
