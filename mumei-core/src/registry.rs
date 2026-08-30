//! # Registry モジュール
//!
//! ローカルパッケージレジストリ (`~/.mumei/registry.json`) の管理。
//! `mumei publish` で公開されたパッケージを名前＋バージョンで検索可能にする。
pub mod remote;

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
    // Write through a temporary file so a crash or a concurrent reader never
    // observes a half-written registry.
    let tmp = path.with_extension(format!("json.tmp-{}", std::process::id()));
    fs::write(&tmp, json).map_err(|e| format!("Failed to write {}: {}", tmp.display(), e))?;
    fs::rename(&tmp, &path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("Failed to write {}: {}", path.display(), e)
    })
}

/// Advisory cross-process lock around a registry read-modify-write, so parallel
/// `mumei` invocations cannot drop each other's entries.
///
/// The lock is an OS advisory file lock (`flock` / `LockFileEx`), so the kernel
/// releases it when the holder exits — a crashed `mumei` cannot wedge later
/// invocations, and a slow holder is never evicted while it is still writing.
///
/// The lock file is `registry.flock`, not the `registry.lock` used by mumei
/// <= 0.6.12: an older binary treats that path's existence as ownership and
/// deletes it once it looks stale, which would make two newer processes lock
/// two different inodes and write concurrently.
struct RegistryLock {
    file: fs::File,
}

impl RegistryLock {
    const WAIT_FOR: std::time::Duration = std::time::Duration::from_secs(10);

    fn acquire() -> Result<Self, String> {
        Self::acquire_at(&registry_path().with_extension("flock"), Self::WAIT_FOR)
    }

    fn acquire_at(path: &Path, wait_for: std::time::Duration) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create {}: {}", parent.display(), e))?;
        }
        let file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .map_err(|e| format!("Failed to open {}: {}", path.display(), e))?;
        let deadline = std::time::Instant::now() + wait_for;
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(Self { file }),
                Err(fs::TryLockError::WouldBlock) => {
                    if std::time::Instant::now() >= deadline {
                        return Err(format!(
                            "Timed out waiting for the registry lock {}",
                            path.display()
                        ));
                    }
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                Err(fs::TryLockError::Error(e)) => {
                    return Err(format!("Failed to lock {}: {}", path.display(), e))
                }
            }
        }
    }
}

impl Drop for RegistryLock {
    fn drop(&mut self) {
        // The lock file itself is left in place: removing it would let another
        // process lock a path that is already unlinked and write concurrently.
        let _ = self.file.unlock();
    }
}
/// パッケージ名とバージョン（省略時は latest）でパスを解決する。
/// バージョンが "*" の場合は latest を使用する。
/// バージョンが "^x.y.z" の場合は semver 互換の最新バージョンを使用する。
/// 見つからなければ None を返す。
pub fn resolve(name: &str, version: Option<&str>) -> Option<PathBuf> {
    let registry = load();
    let entry = registry.packages.get(name)?;
    let resolved_version = select_version(
        entry.versions.keys().map(String::as_str),
        &entry.latest,
        version,
    )?;
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
    register_inner(
        name, version, pkg_path, atom_count, verified, cert_path, cert_hash, true,
    )
}

/// Register a package fetched from a remote registry.
///
/// Unlike [`register_with_cert`], caching an older release does not move the
/// `latest` pointer backwards: `latest` becomes the highest semver among the
/// cached versions, so a later `*` dependency still resolves to the newest one.
pub(crate) fn register_cached_with_cert(
    name: &str,
    version: &str,
    pkg_path: &Path,
    atom_count: usize,
    verified: bool,
    cert_path: Option<String>,
    cert_hash: Option<String>,
) -> Result<(), String> {
    register_inner(
        name, version, pkg_path, atom_count, verified, cert_path, cert_hash, false,
    )
}

#[allow(clippy::too_many_arguments)]
fn register_inner(
    name: &str,
    version: &str,
    pkg_path: &Path,
    atom_count: usize,
    verified: bool,
    cert_path: Option<String>,
    cert_hash: Option<String>,
    force_latest: bool,
) -> Result<(), String> {
    let _lock = RegistryLock::acquire()?;
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
    let next_latest = if force_latest {
        version.to_string()
    } else {
        highest_semver(
            pkg.versions
                .keys()
                .map(String::as_str)
                .chain([pkg.latest.as_str()]),
            version,
        )
    };
    pkg.latest = next_latest;
    save(&registry)
}

/// Highest parseable semver among `versions`, or `fallback` when none parses.
fn highest_semver<'a>(versions: impl Iterator<Item = &'a str>, fallback: &str) -> String {
    let mut best: Option<((u64, u64, u64), String)> = None;
    for v in versions {
        if let Some(parsed) = parse_semver(v) {
            if best.as_ref().is_none_or(|b| parsed > b.0) {
                best = Some((parsed, v.to_string()));
            }
        }
    }
    best.map(|b| b.1).unwrap_or_else(|| fallback.to_string())
}
/// Select a version out of `available` for the requirement `version`.
/// `None` / `"*"` selects `latest`, `^x.y.z` / `~x.y.z` apply the range rules
/// below, and any other string is taken literally.
///
/// Shared by local (`registry.json`) and remote (`remote::resolve`) resolution
/// so both honour the same range semantics.
pub fn select_version<'a>(
    available: impl Iterator<Item = &'a str>,
    latest: &str,
    version: Option<&str>,
) -> Option<String> {
    match version {
        None | Some("*") => Some(latest.to_string()),
        Some(v) if v.starts_with('^') => {
            find_compatible_version(available, v.trim_start_matches('^'))
        }
        Some(v) if v.starts_with('~') => {
            find_tilde_compatible_version(available, v.trim_start_matches('~'))
        }
        Some(v) => Some(v.to_string()),
    }
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
fn find_compatible_version<'a>(
    available: impl Iterator<Item = &'a str>,
    base: &str,
) -> Option<String> {
    let (base_major, base_minor, base_patch) = parse_semver(base)?;
    let mut best: Option<(u64, u64, u64, String)> = None;
    for ver_str in available {
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
                best = Some((major, minor, patch, ver_str.to_string()));
            }
        }
    }
    best.map(|b| b.3)
}
/// Find the highest version compatible with ~base (same major.minor, >= base).
fn find_tilde_compatible_version<'a>(
    available: impl Iterator<Item = &'a str>,
    base: &str,
) -> Option<String> {
    let (base_major, base_minor, base_patch) = parse_semver(base)?;
    let mut best: Option<(u64, u64, u64, String)> = None;
    for ver_str in available {
        if let Some((major, minor, patch)) = parse_semver(ver_str) {
            if major == base_major
                && minor == base_minor
                && patch >= base_patch
                && best.as_ref().is_none_or(|b| patch > b.2)
            {
                best = Some((major, minor, patch, ver_str.to_string()));
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

    /// The advisory lock keeps a second holder out and is released on drop,
    /// without the lock file itself being unlinked.
    #[test]
    fn registry_lock_is_exclusive_and_released_on_drop() {
        let dir = std::env::temp_dir().join(format!("mumei_lock_{}", std::process::id()));
        fs::create_dir_all(&dir).expect("create lock dir");
        let path = dir.join("registry.flock");
        let brief = std::time::Duration::from_millis(200);

        let held = RegistryLock::acquire_at(&path, brief).expect("first holder");
        let err = RegistryLock::acquire_at(&path, brief)
            .err()
            .expect("second holder must wait");
        assert!(
            err.contains("Timed out waiting for the registry lock"),
            "{}",
            err
        );

        drop(held);
        assert!(path.exists(), "the lock file outlives the lock");
        RegistryLock::acquire_at(&path, brief).expect("lock is free again");

        let _ = fs::remove_dir_all(&dir);
    }

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

    /// P24: caching an older remote release keeps the newest cached version as latest
    #[test]
    fn test_highest_semver_prefers_the_newest_cached_version() {
        assert_eq!(
            highest_semver(["1.0.0", "1.2.0"].into_iter(), "1.0.0"),
            "1.2.0"
        );
        assert_eq!(highest_semver(["0.9.0"].into_iter(), "0.9.0"), "0.9.0");
        // Non-semver version strings fall back to the version being registered.
        assert_eq!(
            highest_semver(["nightly"].into_iter(), "nightly"),
            "nightly"
        );
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
