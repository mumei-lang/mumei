//! # Remote registry モジュール（P24）
//!
//! ローカル `~/.mumei/registry.json` に無い name 依存を、設定された
//! リモートレジストリ（HTTP）から取得して `~/.mumei/packages/<name>/<version>/`
//! にキャッシュする。キャッシュ後はローカル registry に登録され、以降の解決は
//! 既存のローカル経路（[`crate::registry::resolve`]）に合流する。
//!
//! リモート解決は opt-in で、`mumei.toml` の `[registry] url` か環境変数
//! `MUMEI_REGISTRY_URL` が設定されている場合にのみ行われる。
//!
//! ## 想定するサーバレイアウト
//!
//! ```text
//! {base}/packages/{name}/index.json          ← バージョン索引
//! {base}/packages/{name}/{version}/{file}    ← index.json の `files[]` に列挙されたファイル
//! {base}/packages/{name}/{version}/.proof-cert.json  ← P5-B 証明書（任意）
//! ```
use serde::Deserialize;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::manifest::RegistryConfig;
use crate::proof_cert;

/// リモート index.json のトップレベル構造
#[derive(Debug, Clone, Deserialize)]
pub struct RemoteIndex {
    /// 最新バージョン（`*` / バージョン省略時に選択される）
    pub latest: String,
    /// バージョン → 配布メタデータ
    pub versions: std::collections::HashMap<String, RemoteVersion>,
}

/// リモート index.json の 1 バージョン分の配布メタデータ
#[derive(Debug, Clone, Deserialize)]
pub struct RemoteVersion {
    /// パッケージルートからの相対パスで列挙されたファイル群
    pub files: Vec<String>,
    /// `.proof-cert.json` の SHA-256（P5-B の `cert_hash` と同じ計算）
    #[serde(default)]
    pub cert_hash: Option<String>,
}

/// リモートから取得してキャッシュしたパッケージ
#[derive(Debug, Clone)]
pub struct FetchedPackage {
    pub version: String,
    pub dir: PathBuf,
    /// 証明書検証が通った場合のみ `Some`
    pub cert_path: Option<PathBuf>,
    pub cert_hash: Option<String>,
    pub atom_count: usize,
    pub verified: bool,
}

/// 1 パッケージあたりに取得するファイル数の上限
const MAX_PACKAGE_FILES: usize = 512;
/// 1 ファイルあたりの取得サイズ上限（バイト）
const MAX_FILE_BYTES: usize = 8 * 1024 * 1024;
/// 証明書ファイル名（P5-B の探索順の 1 番目と同じ）
const CERT_FILE: &str = ".proof-cert.json";

/// リモートレジストリから `name` を解決し、ローカル registry に登録する。
///
/// - `Ok(None)`: リモート未設定、またはリモートに該当 name / version が無い
///   （従来どおりローカル / path / git のみで動作する）
/// - `Err(_)`: 取得エラー、または `strict_imports` 時の証明書エラー
pub fn resolve(
    config: &RegistryConfig,
    name: &str,
    version: Option<&str>,
    strict_imports: bool,
) -> Result<Option<PathBuf>, String> {
    let Some(base_url) = config.effective_url() else {
        return Ok(None);
    };
    let cache_root = crate::manifest::mumei_home().join("packages");
    let Some(fetched) = fetch_package(
        &base_url,
        name,
        version,
        &cache_root,
        config.timeout_ms,
        strict_imports,
    )?
    else {
        return Ok(None);
    };

    super::register_cached_with_cert(
        name,
        &fetched.version,
        &fetched.dir,
        fetched.atom_count,
        fetched.verified,
        fetched
            .cert_path
            .as_ref()
            .map(|p| p.to_string_lossy().to_string()),
        fetched.cert_hash.clone(),
    )?;
    Ok(Some(fetched.dir))
}

/// リモートからパッケージを取得して `cache_root/<name>/<version>/` に展開する。
/// ローカル registry には触れないため、テストから任意のキャッシュ先で実行できる。
pub fn fetch_package(
    base_url: &str,
    name: &str,
    version: Option<&str>,
    cache_root: &Path,
    timeout_ms: u64,
    strict_imports: bool,
) -> Result<Option<FetchedPackage>, String> {
    if !is_valid_package_name(name) {
        return Err(format!(
            "remote registry: invalid package name '{}' (expected ASCII letters, digits, '_' or '-')",
            name
        ));
    }
    let staging = StagingDir::new(cache_root, name)?;
    let result = fetch_into_staging(
        base_url,
        name,
        version,
        cache_root,
        timeout_ms,
        strict_imports,
        staging.path(),
    );
    match result {
        Ok(Some(fetched)) => {
            staging.publish(&fetched.dir)?;
            Ok(Some(rebase_paths(fetched)))
        }
        other => other,
    }
}

/// Download destination that is discarded unless the whole fetch succeeds, so a
/// failed or certificate-less fetch can never leave partial files — or a stale
/// certificate from an earlier fetch — in the published cache directory.
struct StagingDir {
    path: PathBuf,
}

impl StagingDir {
    fn new(cache_root: &Path, name: &str) -> Result<Self, String> {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path = cache_root
            .join(name)
            .join(format!(".staging-{}-{}", std::process::id(), nonce));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path)
            .map_err(|e| format!("remote registry: cannot create {}: {}", path.display(), e))?;
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }

    /// Replace `dest` with the staged content.
    fn publish(&self, dest: &Path) -> Result<(), String> {
        if dest.exists() {
            fs::remove_dir_all(dest).map_err(|e| {
                format!(
                    "remote registry: cannot replace cached {}: {}",
                    dest.display(),
                    e
                )
            })?;
        }
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                format!("remote registry: cannot create {}: {}", parent.display(), e)
            })?;
        }
        fs::rename(&self.path, dest).map_err(|e| {
            format!(
                "remote registry: cannot move {} to {}: {}",
                self.path.display(),
                dest.display(),
                e
            )
        })
    }
}

impl Drop for StagingDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// Rewrite staging paths recorded during the fetch to the published cache dir.
fn rebase_paths(mut fetched: FetchedPackage) -> FetchedPackage {
    if fetched.cert_path.is_some() {
        fetched.cert_path = Some(fetched.dir.join(CERT_FILE));
    }
    fetched
}

#[allow(clippy::too_many_arguments)]
fn fetch_into_staging(
    base_url: &str,
    name: &str,
    version: Option<&str>,
    cache_root: &Path,
    timeout_ms: u64,
    strict_imports: bool,
    staging: &Path,
) -> Result<Option<FetchedPackage>, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_millis(timeout_ms))
        .build()
        .map_err(|e| format!("remote registry: cannot build HTTP client: {}", e))?;

    let base = base_url.trim_end_matches('/');
    warn_if_insecure(base);
    let index_url = format!("{}/packages/{}/index.json", base, name);
    let Some(index_body) = fetch_text(&client, &index_url)? else {
        return Ok(None);
    };
    let index: RemoteIndex = serde_json::from_str(&index_body)
        .map_err(|e| format!("remote registry: cannot parse {}: {}", index_url, e))?;

    let Some(resolved_version) = super::select_version(
        index.versions.keys().map(String::as_str),
        &index.latest,
        version,
    ) else {
        return Ok(None);
    };
    let Some(remote_version) = index.versions.get(&resolved_version) else {
        return Ok(None);
    };
    if remote_version.files.len() > MAX_PACKAGE_FILES {
        return Err(format!(
            "remote registry: package '{}' v{} lists {} files (limit {})",
            name,
            resolved_version,
            remote_version.files.len(),
            MAX_PACKAGE_FILES
        ));
    }

    let pkg_dir = cache_root.join(name).join(&resolved_version);
    let version_url = format!("{}/packages/{}/{}", base, name, resolved_version);

    for rel in &remote_version.files {
        let rel_path = sanitize_relative_path(rel)?;
        let dest = staging.join(&rel_path);
        let file_url = format!("{}/{}", version_url, rel_path.to_string_lossy());
        let Some(body) = fetch_text(&client, &file_url)? else {
            return Err(format!(
                "remote registry: '{}' v{} lists '{}' but {} returned 404",
                name, resolved_version, rel, file_url
            ));
        };
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                format!("remote registry: cannot create {}: {}", parent.display(), e)
            })?;
        }
        fs::write(&dest, body)
            .map_err(|e| format!("remote registry: cannot write {}: {}", dest.display(), e))?;
    }

    let cert_url = format!("{}/{}", version_url, CERT_FILE);
    let cert_body = fetch_text(&client, &cert_url)?;
    let cert_dest = staging.join(CERT_FILE);
    let mut fetched = FetchedPackage {
        version: resolved_version.clone(),
        dir: pkg_dir.clone(),
        cert_path: None,
        cert_hash: None,
        atom_count: 0,
        verified: false,
    };

    match cert_body {
        Some(body) => {
            fs::write(&cert_dest, &body).map_err(|e| {
                format!(
                    "remote registry: cannot write {}: {}",
                    cert_dest.display(),
                    e
                )
            })?;
            match verify_fetched_certificate(
                &cert_dest,
                &body,
                name,
                &resolved_version,
                remote_version.cert_hash.as_deref(),
                strict_imports,
            ) {
                Ok(summary) => {
                    fetched.cert_path = Some(cert_dest);
                    fetched.cert_hash = Some(summary.cert_hash);
                    fetched.atom_count = summary.atom_count;
                    fetched.verified = summary.all_verified;
                }
                Err(e) => {
                    // Drop the certificate so the local resolution path does not
                    // treat an unverifiable file as provenance.
                    let _ = fs::remove_file(&cert_dest);
                    if strict_imports {
                        return Err(e);
                    }
                    eprintln!("  ⚠️  {}", e);
                }
            }
        }
        None => {
            if strict_imports {
                return Err(format!(
                    "Strict imports: remote package '{}' v{} has no proof certificate at {}.",
                    name, resolved_version, cert_url
                ));
            }
            eprintln!(
                "  ⚠️  Remote package '{}' v{} has no proof certificate ({} returned 404).",
                name, resolved_version, cert_url
            );
        }
    }

    Ok(Some(fetched))
}

struct CertificateSummary {
    cert_hash: String,
    atom_count: usize,
    all_verified: bool,
}

/// 取得した証明書を既存の P5-B / P5-C 検証で受け入れ可能か判定する。
/// 新しい verdict 語彙は導入せず、ハッシュ整合性・パース可否・Lean translator
/// メタデータ・パッケージ帰属だけを見る。atom 単位の状態判定は、キャッシュ後に
/// 既存のローカル解決経路（`verify_import_certificate`）が行う。
fn verify_fetched_certificate(
    cert_path: &Path,
    cert_body: &str,
    name: &str,
    version: &str,
    expected_hash: Option<&str>,
    strict_imports: bool,
) -> Result<CertificateSummary, String> {
    let actual_hash = proof_cert::compute_sha256(cert_body);
    if let Some(expected) = expected_hash {
        if expected != actual_hash {
            return Err(format!(
                "remote registry: certificate hash mismatch for '{}' v{} (expected {}, got {})",
                name, version, expected, actual_hash
            ));
        }
    }
    let cert = proof_cert::load_certificate_unvalidated(cert_path).map_err(|e| {
        format!(
            "remote registry: certificate for '{}' v{} could not be parsed: {}",
            name, version, e
        )
    })?;
    // Stale Lean translator metadata is not a fetch-time rejection: the existing
    // import path (`verify_import_certificate`) already downgrades such atoms to
    // "unproven", and `--strict-imports` then fails the build there.
    let translator_ok = match proof_cert::validate_certificate_translator_versions(&cert) {
        Ok(()) => true,
        Err(e) => {
            eprintln!(
                "  ⚠️  Remote certificate for '{}' v{} has invalid Lean translator metadata: {}",
                name, version, e
            );
            false
        }
    };
    match cert.package_name.as_deref() {
        Some(cert_name) if cert_name != name => {
            return Err(format!(
                "remote registry: certificate for '{}' v{} declares package '{}'",
                name, version, cert_name
            ));
        }
        // Without an identity claim the certificate cannot be bound to what was
        // downloaded, so strict imports refuse it instead of trusting the index.
        None if strict_imports => {
            return Err(format!(
                "Strict imports: remote certificate for '{}' v{} does not declare a package name.",
                name, version
            ));
        }
        _ => {}
    }
    match cert.package_version.as_deref() {
        Some(cert_version) if cert_version != version => {
            return Err(format!(
                "remote registry: certificate for '{}' v{} declares version '{}'",
                name, version, cert_version
            ));
        }
        None if strict_imports => {
            return Err(format!(
                "Strict imports: remote certificate for '{}' v{} does not declare a package version.",
                name, version
            ));
        }
        _ => {}
    }
    Ok(CertificateSummary {
        cert_hash: actual_hash,
        atom_count: cert.atoms.len(),
        all_verified: cert.all_verified && translator_ok,
    })
}

/// `Ok(None)` は 404（該当リソース無し）を表す。
fn fetch_text(client: &reqwest::blocking::Client, url: &str) -> Result<Option<String>, String> {
    let response = client
        .get(url)
        .send()
        .map_err(|e| format!("remote registry: GET {} failed: {}", url, e))?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !response.status().is_success() {
        return Err(format!(
            "remote registry: GET {} returned {}",
            url,
            response.status()
        ));
    }
    let too_large = |size: String| {
        Err(format!(
            "remote registry: {} is {} bytes (limit {})",
            url, size, MAX_FILE_BYTES
        ))
    };
    if let Some(len) = response.content_length() {
        if len > MAX_FILE_BYTES as u64 {
            return too_large(len.to_string());
        }
    }
    // Read through a capped reader so a server that lies about (or omits)
    // Content-Length cannot make the client buffer an unbounded body.
    let mut buf = Vec::new();
    response
        .take(MAX_FILE_BYTES as u64 + 1)
        .read_to_end(&mut buf)
        .map_err(|e| format!("remote registry: cannot read body of {}: {}", url, e))?;
    if buf.len() > MAX_FILE_BYTES {
        return too_large(format!("more than {}", MAX_FILE_BYTES));
    }
    String::from_utf8(buf)
        .map(Some)
        .map_err(|_| format!("remote registry: {} is not valid UTF-8", url))
}

/// Over plaintext HTTP the index, package and certificate can all be replaced
/// together, so the certificate chain proves nothing about the origin.
fn warn_if_insecure(base: &str) {
    let Some(rest) = base.strip_prefix("http://") else {
        return;
    };
    let host = rest
        .split('/')
        .next()
        .unwrap_or("")
        .rsplit('@')
        .next()
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("");
    let loopback = host == "localhost" || host == "127.0.0.1" || host == "[::1]" || host == "::1";
    if !loopback {
        eprintln!(
            "  ⚠️  Registry {} uses plaintext HTTP: a network attacker can replace the package and its certificate together. Use https://.",
            base
        );
    }
}

fn is_valid_package_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// index.json が列挙するファイルはそのままファイルシステムに落ちるため、
/// パッケージディレクトリの外へ出る相対パスを拒否する。
fn sanitize_relative_path(rel: &str) -> Result<PathBuf, String> {
    let reject = |reason: &str| {
        Err(format!(
            "remote registry: rejected file '{}' ({})",
            rel, reason
        ))
    };
    if rel.is_empty() {
        return reject("empty path");
    }
    if rel.contains('\\') {
        return reject("backslash separator");
    }
    let path = Path::new(rel);
    if path.is_absolute() {
        return reject("absolute path");
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::Normal(part) => normalized.push(part),
            std::path::Component::CurDir => {}
            _ => return reject("path escapes the package directory"),
        }
    }
    if normalized.as_os_str().is_empty() {
        return reject("empty path");
    }
    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_rejects_parent_and_absolute_paths() {
        assert!(sanitize_relative_path("../evil.mm").is_err());
        assert!(sanitize_relative_path("/etc/passwd").is_err());
        assert!(sanitize_relative_path("src/../../evil.mm").is_err());
        assert!(sanitize_relative_path("").is_err());
        assert_eq!(
            sanitize_relative_path("./src/main.mm").unwrap(),
            PathBuf::from("src/main.mm")
        );
    }

    #[test]
    fn remote_index_selects_versions_like_local_registry() {
        let index: RemoteIndex = serde_json::from_str(
            r#"{"latest":"1.2.0","versions":{
                "1.0.0":{"files":["src/main.mm"]},
                "1.1.0":{"files":["src/main.mm"]},
                "1.2.0":{"files":["src/main.mm"]}}}"#,
        )
        .expect("index parses");
        let keys = || index.versions.keys().map(String::as_str);
        assert_eq!(
            super::super::select_version(keys(), &index.latest, None).as_deref(),
            Some("1.2.0")
        );
        assert_eq!(
            super::super::select_version(keys(), &index.latest, Some("^1.0.0")).as_deref(),
            Some("1.2.0")
        );
        assert_eq!(
            super::super::select_version(keys(), &index.latest, Some("~1.1.0")).as_deref(),
            Some("1.1.0")
        );
        assert_eq!(
            super::super::select_version(keys(), &index.latest, Some("1.0.0")).as_deref(),
            Some("1.0.0")
        );
    }

    #[test]
    fn unset_registry_url_keeps_resolution_local() {
        let config = RegistryConfig::default();
        assert_eq!(config.url, None);
        // Without MUMEI_REGISTRY_URL the config yields no endpoint, so
        // `resolve` short-circuits before touching the network.
        if std::env::var("MUMEI_REGISTRY_URL").is_err() {
            assert_eq!(config.effective_url(), None);
            assert_eq!(resolve(&config, "any_pkg", None, false).unwrap(), None);
        }
    }
}
