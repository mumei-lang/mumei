use super::cache::VerificationCacheEntry;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};

const SCHEMA: &str = "mumei.proof-store-entry/v1";
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug)]
pub struct ProofStore {
    root: PathBuf,
}

#[derive(Debug)]
pub enum StoreLookup {
    Hit(VerificationCacheEntry),
    Miss,
    Invalid(String),
}

#[derive(Serialize, Deserialize)]
struct KeyInputs {
    proof_hash: String,
    z3_version: String,
    mumei_version: String,
}

#[derive(Serialize, Deserialize)]
struct StoreEntry {
    schema: String,
    key: String,
    atom_name: String,
    key_inputs: KeyInputs,
    entry: VerificationCacheEntry,
    content_digest: String,
}

#[derive(Serialize)]
struct UnsignedStoreEntry<'a> {
    schema: &'a str,
    key: &'a str,
    atom_name: &'a str,
    key_inputs: KeyInputsRef<'a>,
    entry: &'a VerificationCacheEntry,
}

#[derive(Serialize)]
struct KeyInputsRef<'a> {
    proof_hash: &'a str,
    z3_version: &'a str,
    mumei_version: &'a str,
}

impl ProofStore {
    pub fn open(dir: Option<&Path>) -> std::io::Result<Self> {
        let root = if let Some(dir) = dir {
            dir.to_path_buf()
        } else if let Some(dir) = std::env::var_os("MUMEI_PROOF_STORE") {
            PathBuf::from(dir)
        } else if let Some(home) = std::env::var_os("HOME") {
            PathBuf::from(home).join(".mumei/proof-store")
        } else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "HOME and MUMEI_PROOF_STORE are unset",
            ));
        };
        create_private_dirs(&root)?;
        let store = Self { root };
        store
            .check_directory_permissions(&store.root)
            .map_err(|reason| std::io::Error::new(std::io::ErrorKind::PermissionDenied, reason))?;
        Ok(store)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn key(proof_hash: &str, z3_version: &str) -> String {
        digest(format!(
            "mumei-proof-store/v1|{proof_hash}|z3:{z3_version}|mumei:{}",
            env!("CARGO_PKG_VERSION")
        ))
    }

    fn path(&self, key: &str) -> PathBuf {
        self.root
            .join("v1")
            .join(&key[..2])
            .join(format!("{key}.json"))
    }

    pub fn lookup(&self, key: &str, proof_hash: &str, z3_version: &str) -> StoreLookup {
        if !valid_key(key) {
            return StoreLookup::Invalid("invalid key".to_string());
        }
        let path = self.path(key);
        if let Err(reason) = self.check_path_permissions(&path, true) {
            return StoreLookup::Invalid(reason);
        }
        let expected_key = Self::key(proof_hash, z3_version);
        if key != expected_key {
            return StoreLookup::Invalid("key mismatch".to_string());
        }
        match std::fs::metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return StoreLookup::Miss;
            }
            Err(error) => return StoreLookup::Invalid(error.to_string()),
            Ok(_) => {}
        }
        let stored = match self.read_valid_entry(&path) {
            Ok(stored) => stored,
            Err(reason) => return StoreLookup::Invalid(reason),
        };
        if stored.key != key
            || stored.key_inputs.proof_hash != proof_hash
            || stored.key_inputs.z3_version != z3_version
        {
            return StoreLookup::Invalid("key_inputs mismatch".to_string());
        }
        StoreLookup::Hit(stored.entry)
    }

    pub fn validate_file(&self, path: &Path) -> Result<(), String> {
        self.check_path_permissions(path, true)?;
        self.read_valid_entry(path).map(|_| ())
    }

    pub fn record(
        &self,
        atom_name: &str,
        proof_hash: &str,
        z3_version: &str,
        entry: &VerificationCacheEntry,
    ) -> std::io::Result<()> {
        self.check_directory_permissions(&self.root)
            .map_err(|reason| std::io::Error::new(std::io::ErrorKind::PermissionDenied, reason))?;
        if entry.result != "verified" || entry.proof_hash != proof_hash {
            return Ok(());
        }
        let key = Self::key(proof_hash, z3_version);
        if matches!(
            self.lookup(&key, proof_hash, z3_version),
            StoreLookup::Hit(_)
        ) {
            return Ok(());
        }
        let path = self.path(&key);
        let parent = path.parent().expect("proof-store key has a parent");
        self.check_path_permissions(&path, false)
            .map_err(|reason| std::io::Error::new(std::io::ErrorKind::PermissionDenied, reason))?;
        create_private_dirs(parent)?;
        self.check_path_permissions(&path, false)
            .map_err(|reason| std::io::Error::new(std::io::ErrorKind::PermissionDenied, reason))?;
        let key_inputs = KeyInputs {
            proof_hash: proof_hash.to_string(),
            z3_version: z3_version.to_string(),
            mumei_version: env!("CARGO_PKG_VERSION").to_string(),
        };
        let content_digest = content_digest(&key, atom_name, &key_inputs, entry);
        let stored = StoreEntry {
            schema: SCHEMA.to_string(),
            key: key.clone(),
            atom_name: atom_name.to_string(),
            key_inputs,
            entry: entry.clone(),
            content_digest,
        };
        let bytes = serde_json::to_vec(&stored)
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let temp = parent.join(format!(
            ".{key}.{}.{}.{sequence}.tmp",
            std::process::id(),
            nanos
        ));
        use std::io::Write;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options.open(&temp)?;
        #[cfg(unix)]
        if let Err(error) = file.set_permissions(std::fs::Permissions::from_mode(0o600)) {
            let _ = std::fs::remove_file(&temp);
            return Err(error);
        }
        if let Err(error) = file.write_all(&bytes) {
            let _ = std::fs::remove_file(&temp);
            return Err(error);
        }
        drop(file);
        if let Err(error) = std::fs::rename(&temp, &path) {
            let _ = std::fs::remove_file(&temp);
            return Err(error);
        }
        Ok(())
    }

    fn read_valid_entry(&self, path: &Path) -> Result<StoreEntry, String> {
        let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
        let stored: StoreEntry =
            serde_json::from_slice(&bytes).map_err(|error| format!("invalid JSON: {error}"))?;
        if stored.schema != SCHEMA {
            return Err("schema mismatch".to_string());
        }
        if !valid_key(&stored.key) {
            return Err("invalid key".to_string());
        }
        if self.path(&stored.key) != path {
            return Err("entry is not at its canonical path".to_string());
        }
        if stored.key_inputs.mumei_version != env!("CARGO_PKG_VERSION")
            || stored.key != Self::key(&stored.key_inputs.proof_hash, &stored.key_inputs.z3_version)
        {
            return Err("key_inputs mismatch".to_string());
        }
        let expected_digest = content_digest(
            &stored.key,
            &stored.atom_name,
            &stored.key_inputs,
            &stored.entry,
        );
        if stored.content_digest != expected_digest {
            return Err("content_digest mismatch".to_string());
        }
        if stored.entry.result != "verified" {
            return Err("entry is not verified".to_string());
        }
        if stored.entry.proof_hash != stored.key_inputs.proof_hash {
            return Err("entry proof_hash mismatch".to_string());
        }
        Ok(stored)
    }

    fn check_path_permissions(&self, path: &Path, check_file: bool) -> Result<(), String> {
        let parent = path
            .parent()
            .ok_or_else(|| format!("invalid proof store path {}", path.display()))?;
        self.check_directory_permissions(parent)?;
        #[cfg(unix)]
        {
            if check_file {
                match std::fs::metadata(path) {
                    Ok(metadata) if metadata.mode() & 0o022 != 0 => Err(shared_path_warning(path)),
                    Ok(_) => Ok(()),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                    Err(error) => Err(error.to_string()),
                }
            } else {
                Ok(())
            }
        }
        #[cfg(not(unix))]
        {
            let _ = check_file;
            Ok(())
        }
    }

    fn check_directory_permissions(&self, path: &Path) -> Result<(), String> {
        #[cfg(unix)]
        {
            let relative = path.strip_prefix(&self.root).map_err(|_| {
                format!(
                    "proof store path {} is outside {}",
                    path.display(),
                    self.root.display()
                )
            })?;
            let mut current = self.root.clone();
            for directory in
                std::iter::once(current.clone()).chain(relative.components().map(|component| {
                    current.push(component);
                    current.clone()
                }))
            {
                match std::fs::metadata(&directory) {
                    Ok(metadata) if metadata.mode() & 0o022 != 0 => {
                        return Err(shared_path_warning(&directory));
                    }
                    Ok(metadata) if !metadata.is_dir() => {
                        return Err(format!(
                            "proof store path {} is not a directory; ignoring it",
                            directory.display()
                        ));
                    }
                    Ok(_) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                    Err(error) => return Err(error.to_string()),
                }
            }
            Ok(())
        }
        #[cfg(not(unix))]
        {
            let _ = path;
            Ok(())
        }
    }
}

#[cfg(unix)]
fn shared_path_warning(path: &Path) -> String {
    format!(
        "proof store {} is writable by other users; ignoring it",
        path.display()
    )
}

fn valid_key(key: &str) -> bool {
    key.len() == 64 && key.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn create_private_dirs(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        builder.mode(0o700);
        builder.create(path)
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(path)
    }
}

fn content_digest(
    key: &str,
    atom_name: &str,
    key_inputs: &KeyInputs,
    entry: &VerificationCacheEntry,
) -> String {
    let unsigned = UnsignedStoreEntry {
        schema: SCHEMA,
        key,
        atom_name,
        key_inputs: KeyInputsRef {
            proof_hash: &key_inputs.proof_hash,
            z3_version: &key_inputs.z3_version,
            mumei_version: &key_inputs.mumei_version,
        },
        entry,
    };
    digest(serde_json::to_vec(&unsigned).expect("serializable proof-store entry"))
}

fn digest(bytes: impl AsRef<[u8]>) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    fn cache_entry(hash: &str) -> VerificationCacheEntry {
        VerificationCacheEntry {
            proof_hash: hash.to_string(),
            result: "verified".to_string(),
            dependencies: Vec::new(),
            type_deps: Vec::new(),
            timestamp: "0s".to_string(),
            skipped_clauses: 0,
            inferred_invariants: Vec::new(),
            cover_results: Vec::new(),
        }
    }

    #[test]
    fn key_is_deterministic_and_tracks_proof_and_solver_versions() {
        assert_eq!(
            ProofStore::key("proof", "z3"),
            ProofStore::key("proof", "z3")
        );
        assert_ne!(
            ProofStore::key("proof", "z3"),
            ProofStore::key("other", "z3")
        );
        assert_ne!(
            ProofStore::key("proof", "z3"),
            ProofStore::key("proof", "other")
        );
    }

    #[test]
    fn concurrent_records_leave_a_valid_entry_without_temporary_files() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(ProofStore::open(Some(dir.path())).unwrap());
        let hash = "proof-hash";
        let z3 = "z3-test";
        let entry = cache_entry(hash);
        let writers: Vec<_> = (0..8)
            .map(|_| {
                let store = Arc::clone(&store);
                let entry = entry.clone();
                thread::spawn(move || store.record("atom", hash, z3, &entry).unwrap())
            })
            .collect();
        for writer in writers {
            writer.join().unwrap();
        }
        let key = ProofStore::key(hash, z3);
        assert!(matches!(store.lookup(&key, hash, z3), StoreLookup::Hit(_)));
        let dir = store.root().join("v1").join(&key[..2]);
        assert_eq!(
            std::fs::read_dir(dir)
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "tmp"))
                .count(),
            0
        );
    }
}
