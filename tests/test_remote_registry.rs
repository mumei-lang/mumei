//! P24: remote package registry resolution tests.
//!
//! These drive `mumei_core::registry::remote::fetch_package` against a local
//! HTTP fixture server (no network access, no mock framework) and assert the
//! certificate handling matches the existing P5-B behaviour: hash mismatch and
//! missing certificates are hard errors under strict imports, and non-strict
//! resolution keeps working without recording unverifiable provenance.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;

use mumei_core::proof_cert::compute_sha256;
use mumei_core::registry::remote;
use mumei_core::verification::{LEAN_BRIDGE_LEMMA_HASH, LEAN_TRANSLATOR_VERSION};

const TIMEOUT_MS: u64 = 5_000;

struct FixtureServer {
    base_url: String,
    shutdown: Arc<AtomicUsize>,
}

impl FixtureServer {
    /// Serve an in-memory `path -> body` map; anything else answers 404.
    fn start(routes: HashMap<String, String>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fixture server");
        let addr = listener.local_addr().expect("fixture server addr");
        let shutdown = Arc::new(AtomicUsize::new(0));
        let stop = Arc::clone(&shutdown);
        thread::spawn(move || {
            for stream in listener.incoming() {
                if stop.load(Ordering::SeqCst) == 1 {
                    break;
                }
                match stream {
                    Ok(stream) => serve(stream, &routes),
                    Err(_) => break,
                }
            }
        });
        Self {
            base_url: format!("http://{}", addr),
            shutdown,
        }
    }
}

impl Drop for FixtureServer {
    fn drop(&mut self) {
        self.shutdown.store(1, Ordering::SeqCst);
    }
}

fn serve(mut stream: TcpStream, routes: &HashMap<String, String>) {
    let mut reader = BufReader::new(stream.try_clone().expect("clone fixture stream"));
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
        return;
    }
    // Drain headers so the client sees a well-formed exchange.
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) if line.trim().is_empty() => break,
            Ok(_) => {}
            Err(_) => return,
        }
    }
    let path = request_line.split_whitespace().nth(1).unwrap_or("/");
    let response = match routes.get(path) {
        Some(body) => format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        ),
        None => "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string(),
    };
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

fn temp_cache(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mumei_remote_registry_{}_{}_{:?}",
        tag,
        std::process::id(),
        thread::current().id()
    ));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("clean stale remote registry cache");
    }
    std::fs::create_dir_all(&dir).expect("create remote registry cache");
    dir
}

fn certificate_json(package: &str, version: &str) -> String {
    certificate_json_with_translator(package, version, LEAN_TRANSLATOR_VERSION)
}

fn certificate_json_with_translator(package: &str, version: &str, translator: &str) -> String {
    let bridge = LEAN_BRIDGE_LEMMA_HASH;
    format!(
        r#"{{
  "version": "1.0",
  "timestamp": "2026-08-29T00:00:00Z",
  "mumei_version": "0.6.12",
  "z3_version": "4.13.0",
  "file": "src/main.mm",
  "package_name": "{package}",
  "package_version": "{version}",
  "all_verified": true,
  "atoms": [
    {{
      "name": "add_one",
      "z3_check_result": "unsat",
      "content_hash": "aaaa",
      "status": "verified",
      "translator_version": "{translator}",
      "bridge_lemma_hash": "{bridge}"
    }},
    {{
      "name": "add_two",
      "z3_check_result": "unsat",
      "content_hash": "bbbb",
      "status": "verified",
      "translator_version": "{translator}",
      "bridge_lemma_hash": "{bridge}"
    }}
  ]
}}"#
    )
}

/// Certificate without `package_name` / `package_version` claims.
fn certificate_json_without_identity() -> String {
    let translator = LEAN_TRANSLATOR_VERSION;
    let bridge = LEAN_BRIDGE_LEMMA_HASH;
    format!(
        r#"{{
  "version": "1.0",
  "timestamp": "2026-08-29T00:00:00Z",
  "mumei_version": "0.6.12",
  "z3_version": "4.13.0",
  "file": "src/main.mm",
  "all_verified": true,
  "atoms": [
    {{
      "name": "add_one",
      "z3_check_result": "unsat",
      "content_hash": "aaaa",
      "status": "verified",
      "translator_version": "{translator}",
      "bridge_lemma_hash": "{bridge}"
    }}
  ]
}}"#
    )
}

const SOURCE: &str = "atom add_one(x: i64) -> i64 { x + 1 }\n";

/// Routes for `remote_pkg` with versions 1.0.0 / 1.1.0 / 1.2.0.
fn package_routes(cert_hash: Option<&str>, with_cert: bool) -> HashMap<String, String> {
    let cert = certificate_json("remote_pkg", "1.2.0");
    let hash_field = match cert_hash {
        Some(h) => format!(", \"cert_hash\": \"{}\"", h),
        None => String::new(),
    };
    let index = format!(
        r#"{{"latest":"1.2.0","versions":{{
            "1.0.0":{{"files":["src/main.mm"]}},
            "1.1.0":{{"files":["src/main.mm"]}},
            "1.2.0":{{"files":["src/main.mm"]{hash_field}}}
        }}}}"#
    );
    let mut routes = HashMap::new();
    routes.insert("/packages/remote_pkg/index.json".to_string(), index);
    for v in ["1.0.0", "1.1.0", "1.2.0"] {
        routes.insert(
            format!("/packages/remote_pkg/{}/src/main.mm", v),
            SOURCE.to_string(),
        );
    }
    if with_cert {
        routes.insert(
            "/packages/remote_pkg/1.2.0/.proof-cert.json".to_string(),
            cert,
        );
    }
    routes
}

#[test]
fn fetches_and_caches_package_with_valid_certificate() {
    let cert = certificate_json("remote_pkg", "1.2.0");
    let hash = compute_sha256(&cert);
    let server = FixtureServer::start(package_routes(Some(&hash), true));
    let cache = temp_cache("valid");

    let fetched = remote::fetch_package(
        &server.base_url,
        "remote_pkg",
        None,
        &cache,
        TIMEOUT_MS,
        true,
    )
    .expect("remote fetch succeeds")
    .expect("package exists remotely");

    assert_eq!(fetched.version, "1.2.0");
    assert_eq!(fetched.dir, cache.join("remote_pkg").join("1.2.0"));
    assert_eq!(
        std::fs::read_to_string(fetched.dir.join("src/main.mm")).unwrap(),
        SOURCE
    );
    assert_eq!(
        fetched.cert_path,
        Some(fetched.dir.join(".proof-cert.json"))
    );
    assert_eq!(fetched.cert_hash.as_deref(), Some(hash.as_str()));
    assert_eq!(fetched.atom_count, 2);
    assert!(fetched.verified);
}

#[test]
fn version_requirements_select_the_same_version_as_local_resolution() {
    let cert = certificate_json("remote_pkg", "1.2.0");
    let hash = compute_sha256(&cert);
    let server = FixtureServer::start(package_routes(Some(&hash), true));
    let cache = temp_cache("versions");

    for (req, expected) in [
        (Some("^1.0.0"), "1.2.0"),
        (Some("~1.1.0"), "1.1.0"),
        (Some("1.0.0"), "1.0.0"),
        (Some("*"), "1.2.0"),
    ] {
        let fetched = remote::fetch_package(
            &server.base_url,
            "remote_pkg",
            req,
            &cache,
            TIMEOUT_MS,
            false,
        )
        .expect("remote fetch succeeds")
        .expect("package exists remotely");
        assert_eq!(fetched.version, expected, "requirement {:?}", req);
    }
}

#[test]
fn unknown_package_and_version_resolve_to_none() {
    let cert = certificate_json("remote_pkg", "1.2.0");
    let hash = compute_sha256(&cert);
    let server = FixtureServer::start(package_routes(Some(&hash), true));
    let cache = temp_cache("unknown");

    assert!(remote::fetch_package(
        &server.base_url,
        "no_such_pkg",
        None,
        &cache,
        TIMEOUT_MS,
        false
    )
    .expect("404 index is not an error")
    .is_none());

    assert!(remote::fetch_package(
        &server.base_url,
        "remote_pkg",
        Some("9.9.9"),
        &cache,
        TIMEOUT_MS,
        false
    )
    .expect("unknown version is not an error")
    .is_none());
}

#[test]
fn certificate_hash_mismatch_is_a_hard_error_under_strict_imports() {
    let server = FixtureServer::start(package_routes(Some("deadbeef"), true));
    let cache = temp_cache("mismatch_strict");

    let err = remote::fetch_package(
        &server.base_url,
        "remote_pkg",
        None,
        &cache,
        TIMEOUT_MS,
        true,
    )
    .expect_err("hash mismatch must fail under strict imports");
    assert!(err.contains("certificate hash mismatch"), "{}", err);
    assert!(!cache.join("remote_pkg/1.2.0/.proof-cert.json").exists());
}

#[test]
fn certificate_hash_mismatch_drops_provenance_without_strict_imports() {
    let server = FixtureServer::start(package_routes(Some("deadbeef"), true));
    let cache = temp_cache("mismatch_lenient");

    let fetched = remote::fetch_package(
        &server.base_url,
        "remote_pkg",
        None,
        &cache,
        TIMEOUT_MS,
        false,
    )
    .expect("non-strict fetch succeeds")
    .expect("package exists remotely");
    assert_eq!(fetched.cert_path, None);
    assert_eq!(fetched.cert_hash, None);
    assert!(!fetched.verified);
    assert!(!fetched.dir.join(".proof-cert.json").exists());
}

#[test]
fn missing_certificate_is_a_hard_error_under_strict_imports() {
    let server = FixtureServer::start(package_routes(None, false));
    let cache = temp_cache("nocert_strict");

    let err = remote::fetch_package(
        &server.base_url,
        "remote_pkg",
        None,
        &cache,
        TIMEOUT_MS,
        true,
    )
    .expect_err("missing certificate must fail under strict imports");
    assert!(err.contains("Strict imports"), "{}", err);

    let cache = temp_cache("nocert_lenient");
    let fetched = remote::fetch_package(
        &server.base_url,
        "remote_pkg",
        None,
        &cache,
        TIMEOUT_MS,
        false,
    )
    .expect("non-strict fetch succeeds")
    .expect("package exists remotely");
    assert_eq!(fetched.cert_path, None);
    assert!(!fetched.verified);
}

#[test]
fn certificate_for_another_package_is_rejected() {
    let mut routes = package_routes(None, false);
    let foreign = certificate_json("other_pkg", "1.2.0");
    routes.insert(
        "/packages/remote_pkg/1.2.0/.proof-cert.json".to_string(),
        foreign,
    );
    let server = FixtureServer::start(routes);
    let cache = temp_cache("foreign_cert");

    let err = remote::fetch_package(
        &server.base_url,
        "remote_pkg",
        None,
        &cache,
        TIMEOUT_MS,
        true,
    )
    .expect_err("foreign certificate must fail under strict imports");
    assert!(err.contains("declares package 'other_pkg'"), "{}", err);
}

#[test]
fn stale_translator_metadata_caches_the_package_but_not_as_verified() {
    let cert =
        certificate_json_with_translator("remote_pkg", "1.2.0", "mumei-lean-translator-ir-v0");
    let hash = compute_sha256(&cert);
    let mut routes = package_routes(Some(&hash), false);
    routes.insert(
        "/packages/remote_pkg/1.2.0/.proof-cert.json".to_string(),
        cert,
    );
    let server = FixtureServer::start(routes);
    let cache = temp_cache("stale_translator");

    let fetched = remote::fetch_package(
        &server.base_url,
        "remote_pkg",
        None,
        &cache,
        TIMEOUT_MS,
        true,
    )
    .expect("stale translator metadata is handled by the import path, not the fetch")
    .expect("package exists remotely");
    // The existing import path downgrades such atoms to "unproven", so the
    // cached package must not be recorded as verified provenance.
    assert!(!fetched.verified);
    assert!(fetched.cert_path.is_some());
}

/// End-to-end: `mumei add <name>` fetches from the configured remote registry,
/// verifies the certificate, caches under `~/.mumei/packages/<name>/<version>/`
/// and records the resolved version in mumei.toml.
#[test]
fn mumei_add_fetches_and_caches_from_the_remote_registry() {
    let cert = certificate_json("remote_pkg", "1.2.0");
    let hash = compute_sha256(&cert);
    let server = FixtureServer::start(package_routes(Some(&hash), true));

    let home = temp_cache("cli_home");
    let project = temp_cache("cli_project");
    std::fs::write(
        project.join("mumei.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\n",
    )
    .expect("write project manifest");

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_mumei"))
        .args(["add", "remote_pkg"])
        .current_dir(&project)
        .env("HOME", &home)
        .env("MUMEI_REGISTRY_URL", &server.base_url)
        .output()
        .expect("run mumei add");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "stdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        stdout.contains("Fetched from remote registry"),
        "stdout: {stdout}"
    );

    let cached = home.join(".mumei/packages/remote_pkg/1.2.0");
    assert_eq!(
        std::fs::read_to_string(cached.join("src/main.mm")).unwrap(),
        SOURCE
    );
    assert!(cached.join(".proof-cert.json").exists());

    let registry_json =
        std::fs::read_to_string(home.join(".mumei/registry.json")).expect("registry.json written");
    assert!(registry_json.contains("remote_pkg"), "{registry_json}");
    assert!(registry_json.contains(&hash), "{registry_json}");

    let manifest = std::fs::read_to_string(project.join("mumei.toml")).unwrap();
    assert!(
        manifest.contains("remote_pkg = \"1.2.0\""),
        "manifest: {manifest}"
    );
}

#[test]
fn certificate_without_package_identity_is_rejected_under_strict_imports() {
    let cert = certificate_json_without_identity();
    let hash = compute_sha256(&cert);
    let mut routes = package_routes(Some(&hash), false);
    routes.insert(
        "/packages/remote_pkg/1.2.0/.proof-cert.json".to_string(),
        cert,
    );
    let server = FixtureServer::start(routes);

    let err = remote::fetch_package(
        &server.base_url,
        "remote_pkg",
        None,
        &temp_cache("anon_cert_strict"),
        TIMEOUT_MS,
        true,
    )
    .expect_err("an unattributed certificate must fail under strict imports");
    assert!(err.contains("does not declare a package name"), "{}", err);

    let fetched = remote::fetch_package(
        &server.base_url,
        "remote_pkg",
        None,
        &temp_cache("anon_cert_lenient"),
        TIMEOUT_MS,
        false,
    )
    .expect("non-strict fetch succeeds")
    .expect("package exists remotely");
    // Non-strict resolution keeps the certificate: the existing import path
    // still decides per-atom status from it.
    assert!(fetched.cert_path.is_some());
}

/// A version that stops publishing its certificate must not keep the
/// certificate an earlier fetch cached, or the import path would still treat
/// the package as certified.
#[test]
fn refetching_without_a_certificate_drops_the_cached_one() {
    let cache = temp_cache("cert_disappears");
    let cert = certificate_json("remote_pkg", "1.2.0");
    let hash = compute_sha256(&cert);

    let with_cert = FixtureServer::start(package_routes(Some(&hash), true));
    let first = remote::fetch_package(
        &with_cert.base_url,
        "remote_pkg",
        None,
        &cache,
        TIMEOUT_MS,
        false,
    )
    .expect("first fetch succeeds")
    .expect("package exists remotely");
    assert!(first.dir.join(".proof-cert.json").exists());
    drop(with_cert);

    let without_cert = FixtureServer::start(package_routes(None, false));
    let second = remote::fetch_package(
        &without_cert.base_url,
        "remote_pkg",
        None,
        &cache,
        TIMEOUT_MS,
        false,
    )
    .expect("second fetch succeeds")
    .expect("package exists remotely");
    assert_eq!(second.cert_path, None);
    assert!(!second.dir.join(".proof-cert.json").exists());
}

#[test]
fn a_failed_fetch_leaves_no_partial_cache() {
    let mut routes = HashMap::new();
    routes.insert(
        "/packages/remote_pkg/index.json".to_string(),
        r#"{"latest":"1.0.0","versions":{"1.0.0":{"files":["src/main.mm","src/missing.mm"]}}}"#
            .to_string(),
    );
    routes.insert(
        "/packages/remote_pkg/1.0.0/src/main.mm".to_string(),
        SOURCE.to_string(),
    );
    let server = FixtureServer::start(routes);
    let cache = temp_cache("partial");

    let err = remote::fetch_package(
        &server.base_url,
        "remote_pkg",
        None,
        &cache,
        TIMEOUT_MS,
        false,
    )
    .expect_err("a missing listed file fails the fetch");
    assert!(err.contains("returned 404"), "{}", err);
    assert!(!cache.join("remote_pkg/1.0.0").exists());
    // Only the (already removed) staging directory may have existed.
    let leftovers: Vec<_> = std::fs::read_dir(cache.join("remote_pkg"))
        .map(|entries| entries.filter_map(Result::ok).map(|e| e.path()).collect())
        .unwrap_or_default();
    assert!(leftovers.is_empty(), "leftovers: {:?}", leftovers);
}

/// Caching an older release must not move the local `latest` pointer backwards.
#[test]
fn caching_an_older_version_does_not_demote_latest() {
    let cert = certificate_json("remote_pkg", "1.2.0");
    let hash = compute_sha256(&cert);
    let server = FixtureServer::start(package_routes(Some(&hash), true));

    let home = temp_cache("latest_home");
    let new_project = |tag: &str| {
        let project = temp_cache(tag);
        std::fs::write(
            project.join("mumei.toml"),
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\n",
        )
        .expect("write project manifest");
        project
    };

    let mumei_add = |project: &PathBuf, args: &[&str]| {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_mumei"))
            .args(args)
            .current_dir(project)
            .env("HOME", &home)
            .env("MUMEI_REGISTRY_URL", &server.base_url)
            .output()
            .expect("run mumei add");
        assert!(
            output.status.success(),
            "{:?} failed: {}{}",
            args,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    };
    mumei_add(&new_project("latest_project_a"), &["add", "remote_pkg"]);
    mumei_add(
        &new_project("latest_project_b"),
        &["add", "remote_pkg", "--version", "1.0.0"],
    );

    // A range fetches remotely and records the concrete version it selected.
    let ranged = new_project("range_project");
    mumei_add(&ranged, &["add", "remote_pkg", "--version", "^1.1.0"]);
    let manifest = std::fs::read_to_string(ranged.join("mumei.toml")).expect("manifest readable");
    assert!(
        manifest.contains("remote_pkg = \"1.2.0\""),
        "manifest: {manifest}"
    );

    let registry: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(home.join(".mumei/registry.json")).expect("registry.json written"),
    )
    .expect("registry.json parses");
    let pkg = &registry["packages"]["remote_pkg"];
    assert_eq!(pkg["latest"], "1.2.0", "registry: {}", registry);
    assert!(pkg["versions"]["1.0.0"].is_object(), "registry: {registry}");
}

#[test]
fn listed_files_cannot_escape_the_package_directory() {
    let mut routes = HashMap::new();
    routes.insert(
        "/packages/remote_pkg/index.json".to_string(),
        r#"{"latest":"1.0.0","versions":{"1.0.0":{"files":["../../evil.mm"]}}}"#.to_string(),
    );
    let server = FixtureServer::start(routes);
    let cache = temp_cache("traversal");

    let err = remote::fetch_package(
        &server.base_url,
        "remote_pkg",
        None,
        &cache,
        TIMEOUT_MS,
        false,
    )
    .expect_err("path traversal must be rejected");
    assert!(err.contains("rejected file"), "{}", err);
    assert!(!cache.join("evil.mm").exists());
}
