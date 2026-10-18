//! Persistent dependency indexes partitioned by source privacy.

use crate::context_index::IndexedContext;
use alloc::borrow::ToOwned;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Bumped when the stored entry layout changes.
pub(crate) const SCHEMA: u32 = 3;

/// A package index loaded from the store.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct Stored {
    pub(crate) schema: u32,
    pub(crate) canonicalization: String,
    /// Content blake3 of each source input, keyed by package-relative path.
    /// Mutable entries include the package manifest under `Cargo.toml`.
    pub(crate) files: BTreeMap<String, String>,
    pub(crate) records: Vec<IndexedContext>,
}

#[derive(serde::Serialize)]
struct Entry<'a> {
    schema: u32,
    canonicalization: &'a str,
    files: &'a BTreeMap<String, String>,
    records: &'a [IndexedContext],
}

/// The outcome of a cache lookup.
#[derive(Debug)]
pub(crate) enum Load {
    /// An intact entry, validated by the caller.
    Stored(Stored),
    /// No entry yet.
    Missing,
    /// An unreadable or unparseable entry, safe to rebuild over.
    Corrupt,
    /// An I/O problem, the entry may be intact.
    Io(std::io::Error),
}

/// Package indexes under one cache directory.
pub(crate) struct Store {
    root: PathBuf,
}

impl Store {
    /// The per-user cache directory, honoring `XDG_CACHE_HOME`.
    pub(crate) fn default_root() -> PathBuf {
        match std::env::var_os("XDG_CACHE_HOME") {
            Some(dir) if !dir.is_empty() => PathBuf::from(dir).join("dejadoc"),
            _ => {
                let home = std::env::var_os("HOME").map_or_else(PathBuf::new, PathBuf::from);
                home.join(".cache").join("dejadoc")
            }
        }
    }

    /// Open `root`, creating it when absent.
    pub(crate) fn open(root: &Path) -> std::io::Result<Self> {
        std::fs::create_dir_all(root)?;
        Ok(Self {
            root: root.to_path_buf(),
        })
    }

    /// Load the entry under `dir`/`name.json`.
    pub(crate) fn load(&self, dir: &str, name: &str) -> Load {
        let path = self.root.join(dir).join(format!("{name}.json"));
        match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<Stored>(&bytes) {
                Ok(stored) => Load::Stored(stored),
                Err(_) => Load::Corrupt,
            },
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Load::Missing,
            Err(err) => Load::Io(err),
        }
    }

    /// Write `records` to `dir`/`name.json`, replacing any entry atomically.
    pub(crate) fn save(
        &self,
        dir: &str,
        name: &str,
        canonicalization: &str,
        files: &BTreeMap<String, String>,
        records: &[IndexedContext],
    ) -> std::io::Result<()> {
        let dir = self.root.join(dir);
        std::fs::create_dir_all(&dir)?;
        let payload = Entry {
            schema: SCHEMA,
            canonicalization,
            files,
            records,
        };
        let target = dir.join(format!("{name}.json"));
        let mut temp = tempfile::NamedTempFile::new_in(dir)?;
        {
            let mut writer = std::io::BufWriter::new(temp.as_file_mut());
            serde_json::to_writer(&mut writer, &payload)
                .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
            writer.flush()?;
        }
        temp.persist(target).map_err(|error| error.error)?;
        Ok(())
    }
}

/// A file exercising the canonicalizer's rename, drift and context paths.
const SENTINEL: &str = r"
pub mod alpha {
    use core::mem;
    pub struct Beta {
        pub gamma: u32,
    }
    impl Beta {
        pub fn delta(&self, input: u32) -> u32 {
            let value = input + 1u32;
            match value {
                0 => self.gamma,
                _ => mem::swap(value, 2u32),
            }
        }
    }
    pub fn epsilon(value: u32) -> u32 {
        let zeta: u8 = 0;
        if value > 1u32 { value - 1u32 } else { u32::from(zeta) }
    }
}
fn main() {
    let beta = alpha::Beta { gamma: 3u32 };
    beta.delta(1u32);
    alpha::epsilon(2u32);
}
";

/// A fingerprint of canonicalization output and the crate version.
pub(crate) fn canonicalization_fingerprint() -> String {
    let Ok(file) = syn::parse_file(SENTINEL) else {
        return env!("CARGO_PKG_VERSION").to_owned();
    };
    let tokens = syn_canon::canonicalize(file).to_string();
    format!(
        "{}:{}",
        blake3::hash(tokens.as_bytes()).to_hex(),
        env!("CARGO_PKG_VERSION")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn records(n: usize) -> Vec<IndexedContext> {
        (0..n)
            .map(|i| IndexedContext {
                form: format!("form-{i}"),
                tokens: i + 1,
                site: crate::ContextSite {
                    file: "src/lib.rs".to_owned(),
                    line: u32::try_from(i).unwrap_or(u32::MAX) + 1,
                    column: 1,
                    end: u32::try_from(i).unwrap_or(u32::MAX) + 2,
                    end_column: 2,
                    item: format!("pkg::f{i}"),
                    kind: crate::ContextKind::FunctionBody,
                },
                module: format!("pkg::f{i}"),
                crate_name: "pkg".to_owned(),
                crate_root: "/tmp/pkg/src/lib.rs".to_owned(),
                access: crate::ContextAccess::PublicFunction,
                conditions: Vec::new(),
                extraction_notes: Vec::new(),
                api_path: None,
            })
            .collect()
    }

    fn map(n: usize) -> BTreeMap<String, String> {
        (0..n)
            .map(|i| (format!("src/f{i}.rs"), format!("hash-{i}")))
            .collect()
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let records = records(3);
        store
            .save(
                "public/crates.io",
                "alpha-1.0.0",
                &canonicalization_fingerprint(),
                &map(3),
                &records,
            )
            .unwrap();
        match store.load("public/crates.io", "alpha-1.0.0") {
            Load::Stored(stored) => {
                assert_eq!(stored.schema, SCHEMA);
                assert_eq!(stored.canonicalization, canonicalization_fingerprint());
                assert_eq!(stored.files, map(3));
                assert_eq!(stored.records.len(), 3);
            }
            other => panic!("expected a stored entry, got {other:?}"),
        }
        assert!(matches!(
            store.load("public/crates.io", "alpha-1.0.1"),
            Load::Missing
        ));
        let raw = dir.path().join("public/crates.io/alpha-1.0.0.json");
        assert!(raw.is_file());
        let temp = dir.path().join("public/crates.io/.alpha-1.0.0.json.tmp");
        assert!(!temp.exists(), "temp file must not survive a save");
    }

    #[test]
    fn corrupt_entry_reports_corrupt_and_rebuilds() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        std::fs::create_dir_all(dir.path().join("private/path-a")).unwrap();
        std::fs::write(dir.path().join("private/path-a/beta.json"), "not json").unwrap();
        let records = records(1);
        store
            .save(
                "private/path-a",
                "beta",
                &canonicalization_fingerprint(),
                &map(1),
                &records,
            )
            .unwrap();
        assert!(matches!(
            store.load("private/path-a", "beta"),
            Load::Stored(stored) if stored.schema == SCHEMA
        ));
    }

    #[test]
    fn truncated_entry_reports_corrupt() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let records = records(1);
        store
            .save(
                "private/path-a",
                "beta",
                &canonicalization_fingerprint(),
                &map(1),
                &records,
            )
            .unwrap();
        let path = dir.path().join("private/path-a/beta.json");
        let bytes = std::fs::read(&path).unwrap();
        std::fs::write(&path, &bytes[..bytes.len() / 2]).unwrap();
        assert!(matches!(
            store.load("private/path-a", "beta"),
            Load::Corrupt
        ));
    }

    #[test]
    fn save_replaces_an_existing_entry() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let one = records(1);
        store
            .save("public/crates.io", "alpha-1.0.0", "c1", &map(1), &one)
            .unwrap();
        let two = records(2);
        store
            .save("public/crates.io", "alpha-1.0.0", "c2", &map(2), &two)
            .unwrap();
        match store.load("public/crates.io", "alpha-1.0.0") {
            Load::Stored(stored) => {
                assert_eq!(stored.canonicalization, "c2");
                assert_eq!(stored.records.len(), 2);
            }
            other => panic!("expected a stored entry, got {other:?}"),
        }
    }
}
