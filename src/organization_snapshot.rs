//! Cargo package and target discovery for a pinned organization snapshot.
//!
//! Manifests resolve through bounded offline `cargo metadata` from a trusted
//! cwd, isolated `CARGO_HOME` and `CARGO_TARGET_DIR` and a scrubbed
//! environment, and only paths inside the snapshot boundary are indexed.

use alloc::collections::BTreeSet;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::time::Duration;
use std::path::{Path, PathBuf};

/// The snapshot root inside a commit directory.
pub(crate) const SNAPSHOT_TREE: &str = "tree";

/// Deadline for one bounded cargo metadata invocation.
const METADATA_DEADLINE: Duration = Duration::from_secs(60);
/// Trusted environment variables kept for the metadata subprocess.
const TRUSTED_ENVIRONMENT: [&str; 11] = [
    "PATH",
    "HOME",
    "HOMEDRIVE",
    "HOMEPATH",
    "SYSTEMROOT",
    "WINDIR",
    "USERPROFILE",
    "APPDATA",
    "LOCALAPPDATA",
    "RUSTUP_HOME",
    "RUSTUP_TOOLCHAIN",
];

/// One discovered package and its targets to scan.
#[derive(Debug, Clone)]
pub(crate) struct PackageTargets {
    /// Cargo package name.
    pub(crate) name: String,
    /// Absolute path of the package manifest.
    pub(crate) manifest: PathBuf,
    /// Targets to scan, in cargo metadata order.
    pub(crate) targets: Vec<crate::discover::Target>,
}

/// Every package discovered in one snapshot, plus the explicit failures.
#[derive(Debug, Clone)]
pub(crate) struct Discovery {
    /// Discovered packages, in manifest enumeration order.
    pub(crate) packages: Vec<PackageTargets>,
    /// One entry per manifest cargo could not resolve, anchored at its
    /// snapshot-relative path.
    pub(crate) failures: Vec<String>,
}

/// Discover every `Cargo.toml` under the snapshot root `commit/tree`, deduplicate the
/// manifests returned through workspace discovery and resolve each package with bounded
/// offline cargo metadata.
///
/// `commit` must be private and hold nothing cargo reads, since it becomes `CARGO_HOME`,
/// where cargo's workspace search stops.
pub(crate) fn discover(commit: &Path, all_targets: bool) -> Discovery {
    let mut discovery = Discovery {
        packages: Vec::new(),
        failures: Vec::new(),
    };
    let resolved = std::fs::canonicalize(commit).and_then(|home| {
        let root = std::fs::canonicalize(home.join(SNAPSHOT_TREE))?;
        Ok((home, root))
    });
    let (home, root) = match resolved {
        Ok(resolved) => resolved,
        Err(error) => {
            discovery
                .failures
                .push(format!(".: the snapshot root cannot be resolved {error}"));
            return discovery;
        }
    };
    let isolation = match Isolation::new(home) {
        Ok(isolation) => isolation,
        Err(failure) => {
            discovery.failures.push(failure);
            return discovery;
        }
    };
    let mut visited = BTreeSet::new();
    let mut manifests = BTreeSet::new();
    visited.insert(root.clone());
    walk_snapshot(
        &root,
        &root,
        &mut visited,
        &mut manifests,
        &mut discovery.failures,
    );
    let mut covered_manifests = BTreeSet::new();
    let mut covered_roots = BTreeSet::new();
    for manifest in manifests {
        if covered_manifests.contains(&manifest) || covered_roots.contains(&manifest) {
            continue;
        }
        let metadata = match invoke_metadata(&isolation, &manifest) {
            Ok(metadata) => metadata,
            Err(error) => {
                discovery
                    .failures
                    .push(qualify(&root, &manifest, &error.to_string()));
                continue;
            }
        };
        let Ok(workspace_root) = std::fs::canonicalize(&metadata.workspace_root) else {
            discovery.failures.push(qualify(
                &root,
                &manifest,
                "the workspace root cannot be resolved",
            ));
            continue;
        };
        if !workspace_root.starts_with(&root) {
            discovery.failures.push(qualify(
                &root,
                &manifest,
                "the workspace root resolves outside the snapshot",
            ));
            continue;
        }
        covered_roots.insert(workspace_root.join("Cargo.toml"));
        for package in &metadata.packages {
            let Some(package) = package_targets(
                &root,
                &manifest,
                package,
                all_targets,
                &mut covered_manifests,
                &mut discovery.failures,
            ) else {
                continue;
            };
            discovery.packages.push(package);
        }
    }
    discovery
}

/// The directories cargo runs with, none of which another user or the snapshot controls.
struct Isolation {
    /// The commit directory, `CARGO_HOME`.
    home: PathBuf,
    /// A private working directory, absent where the root-owned `/` serves.
    working: Option<tempfile::TempDir>,
    /// The isolated `CARGO_TARGET_DIR`.
    target: tempfile::TempDir,
}

impl Isolation {
    /// Create the private directories, or the coverage failure naming the one that failed.
    fn new(home: PathBuf) -> Result<Self, String> {
        let make = |what: &str| {
            tempfile::tempdir().map_err(|error| format!(".: the {what} cannot be created {error}"))
        };
        Ok(Self {
            home,
            working: if cfg!(unix) {
                None
            } else {
                Some(make("private working directory")?)
            },
            target: make("isolated cargo target directory")?,
        })
    }

    /// The working directory, whose ancestors supply no cargo or rustup configuration.
    fn working(&self) -> &Path {
        self.working
            .as_ref()
            .map_or(Path::new("/"), tempfile::TempDir::path)
    }
}

/// The confined targets of one metadata package, recording a failure for every escape.
fn package_targets(
    root: &Path,
    manifest: &Path,
    package: &cargo_metadata::Package,
    all_targets: bool,
    covered_manifests: &mut BTreeSet<PathBuf>,
    failures: &mut Vec<String>,
) -> Option<PackageTargets> {
    let Ok(manifest_path) = manifest_identity(package.manifest_path.as_std_path()) else {
        failures.push(qualify(
            root,
            manifest,
            "a package manifest cannot be resolved",
        ));
        return None;
    };
    covered_manifests.insert(manifest_path.clone());
    let confined = std::fs::canonicalize(package.manifest_path.as_std_path())
        .is_ok_and(|target| target.starts_with(root));
    if !manifest_path.starts_with(root) || !confined {
        failures.push(qualify(
            root,
            manifest,
            "a workspace member resolves outside the snapshot",
        ));
        return None;
    }
    let mut targets = Vec::new();
    for target in &package.targets {
        match std::fs::canonicalize(&target.src_path) {
            Ok(src) if src.starts_with(root) => targets.push(crate::discover::Target {
                name: target.name.clone(),
                package: package.name.to_string(),
                src,
                doc: crate::discover::scan_target(&target.kind, all_targets),
                library: crate::discover::library_target(&target.kind),
            }),
            _ => failures.push(qualify(
                root,
                &manifest_path,
                &format!("target {} resolves outside the snapshot", target.name),
            )),
        }
    }
    Some(PackageTargets {
        name: package.name.to_string(),
        manifest: manifest_path,
        targets,
    })
}

/// The `Cargo.toml` spelling inside its canonical owning directory, the path Cargo accepts
/// and the identity that deduplicates aliases.
fn manifest_identity(manifest: &Path) -> std::io::Result<PathBuf> {
    let parent = manifest
        .parent()
        .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotFound))?;
    Ok(std::fs::canonicalize(parent)?.join("Cargo.toml"))
}

/// Run the bounded offline cargo metadata job for one manifest.
fn invoke_metadata(
    isolation: &Isolation,
    manifest: &Path,
) -> Result<cargo_metadata::Metadata, ManifestError> {
    let mut command = std::process::Command::new("cargo");
    command
        .args([
            "metadata",
            "--no-deps",
            "--format-version",
            "1",
            "--offline",
            "--manifest-path",
        ])
        .arg(manifest)
        .current_dir(isolation.working())
        .env_clear()
        .env("CARGO_HOME", &isolation.home)
        .env("CARGO_TARGET_DIR", isolation.target.path());
    for key in TRUSTED_ENVIRONMENT {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    let id = crate::dependencies::next_proc_id();
    let output = crate::dependencies::run_bounded(&mut command, METADATA_DEADLINE, &id)
        .map_err(|source| ManifestError::Process { source })?;
    if !output.status.success() {
        return Err(ManifestError::Status {
            status: output.status,
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    serde_json::from_slice(&output.stdout).map_err(|source| ManifestError::Output { source })
}

/// One manifest the bounded cargo metadata job could not resolve.
#[derive(Debug, thiserror::Error)]
enum ManifestError {
    /// The bounded subprocess itself failed.
    #[error("{source}")]
    Process {
        #[source]
        source: crate::dependencies::ProcessError,
    },
    /// cargo finished with a nonzero status, for a malformed manifest or an
    /// unresolvable workspace.
    #[error("cargo metadata finished with {status}: {stderr}")]
    Status {
        status: std::process::ExitStatus,
        stderr: String,
    },
    /// cargo succeeded but returned no parseable metadata.
    #[error("cargo metadata returned no parseable metadata")]
    Output {
        #[source]
        source: serde_json::Error,
    },
}

/// Walk the snapshot tree, queue every canonical `Cargo.toml` once, follow
/// each confined link target once and record an explicit failure for every
/// dangling, outside, special or unreadable entry.
fn walk_snapshot(
    root: &Path,
    dir: &Path,
    visited: &mut BTreeSet<PathBuf>,
    manifests: &mut BTreeSet<PathBuf>,
    failures: &mut Vec<String>,
) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) => {
            failures.push(qualify(
                root,
                dir,
                &format!("the directory cannot be enumerated {error}"),
            ));
            return;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                failures.push(qualify(
                    root,
                    dir,
                    &format!("an entry cannot be read {error}"),
                ));
                continue;
            }
        };
        let path = entry.path();
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(error) => {
                failures.push(qualify(
                    root,
                    &path,
                    &format!("the entry type cannot be read {error}"),
                ));
                continue;
            }
        };
        if file_type.is_symlink() {
            let target = match std::fs::canonicalize(&path) {
                Ok(target) => target,
                Err(error) => {
                    failures.push(qualify(
                        root,
                        &path,
                        &format!("the symlink cannot be resolved {error}"),
                    ));
                    continue;
                }
            };
            if !target.starts_with(root) {
                failures.push(qualify(
                    root,
                    &path,
                    "the symlink target is outside the snapshot",
                ));
                continue;
            }
            if entry.file_name() == "Cargo.toml" && !target.is_dir() {
                queue_manifest(root, &path, manifests, failures);
            } else if target.is_dir() && visited.insert(target.clone()) {
                walk_snapshot(root, &target, visited, manifests, failures);
            }
        } else if file_type.is_dir() {
            if visited.insert(path.clone()) {
                walk_snapshot(root, &path, visited, manifests, failures);
            }
        } else if file_type.is_file() {
            if entry.file_name() == "Cargo.toml" {
                queue_manifest(root, &path, manifests, failures);
            }
        } else {
            failures.push(qualify(
                root,
                &path,
                "an unsupported special file was encountered",
            ));
        }
    }
}

/// Queue one manifest by its identity, recording a failure when its directory is unresolvable.
fn queue_manifest(
    root: &Path,
    manifest: &Path,
    manifests: &mut BTreeSet<PathBuf>,
    failures: &mut Vec<String>,
) {
    match manifest_identity(manifest) {
        Ok(identity) => {
            manifests.insert(identity);
        }
        Err(error) => failures.push(qualify(
            root,
            manifest,
            &format!("the manifest cannot be resolved {error}"),
        )),
    }
}

/// One coverage failure entry, anchored at the snapshot-relative manifest.
fn qualify(root: &Path, manifest: &Path, reason: &str) -> String {
    let anchor = manifest
        .strip_prefix(root)
        .unwrap_or(manifest)
        .to_string_lossy();
    if anchor.is_empty() {
        format!(".: {reason}")
    } else {
        format!("{anchor}: {reason}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn standalone(dir: &Path, name: &str) {
        write(
            &dir.join("Cargo.toml"),
            &format!(
                "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[workspace]\n"
            ),
        );
        write(&dir.join("src/lib.rs"), "pub fn f() {}\n");
    }

    fn member(dir: &Path, name: &str) {
        write(
            &dir.join("Cargo.toml"),
            &format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
        );
        write(&dir.join("src/lib.rs"), "pub fn f() {}\n");
    }

    #[test]
    fn independent_and_nested_workspaces_are_discovered_without_duplicate_manifests() {
        let dir = tempfile::tempdir().unwrap();
        let root = &dir.path().join(SNAPSHOT_TREE);
        write(
            &root.join("ws1/Cargo.toml"),
            "[workspace]\nmembers = [\"crates/a\", \"crates/b\"]\nresolver = \"2\"\n",
        );
        member(&root.join("ws1/crates/a"), "a");
        member(&root.join("ws1/crates/b"), "b");
        write(
            &root.join("ws1/nested/Cargo.toml"),
            "[workspace]\nmembers = [\"n1\", \"n2\"]\nresolver = \"2\"\n",
        );
        member(&root.join("ws1/nested/n1"), "n1");
        member(&root.join("ws1/nested/n2"), "n2");
        write(
            &root.join("ws2/Cargo.toml"),
            "[workspace]\nmembers = [\"sub\"]\nresolver = \"2\"\n[package]\nname = \"standalone\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        );
        write(&root.join("ws2/src/lib.rs"), "pub fn f() {}\n");
        member(&root.join("ws2/sub"), "sub");

        let discovery = discover(dir.path(), false);
        assert!(
            discovery.failures.is_empty(),
            "unexpected failures {:#?}",
            discovery.failures
        );
        let mut packages: Vec<_> = discovery
            .packages
            .iter()
            .map(|package| {
                (
                    package.name.as_str(),
                    package
                        .manifest
                        .strip_prefix(root)
                        .unwrap()
                        .to_string_lossy(),
                )
            })
            .collect();
        packages.sort_unstable();
        assert_eq!(
            packages,
            [
                ("a", "ws1/crates/a/Cargo.toml".into()),
                ("b", "ws1/crates/b/Cargo.toml".into()),
                ("n1", "ws1/nested/n1/Cargo.toml".into()),
                ("n2", "ws1/nested/n2/Cargo.toml".into()),
                ("standalone", "ws2/Cargo.toml".into()),
                ("sub", "ws2/sub/Cargo.toml".into()),
            ]
        );
        for package in &discovery.packages {
            assert_eq!(package.targets.len(), 1);
            assert!(package.targets[0].doc);
            assert!(package.targets[0].library);
        }
    }

    #[test]
    fn custom_targets_keep_their_paths_kinds_and_doc_flags() {
        let dir = tempfile::tempdir().unwrap();
        let root = &dir.path().join(SNAPSHOT_TREE);
        let crate_dir = root.join("crate");
        write(
            &crate_dir.join("Cargo.toml"),
            "[package]\nname = \"custom\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[workspace]\n[lib]\nname = \"custom_lib\"\npath = \"elsewhere/lib.rs\"\n[[bin]]\nname = \"sibling\"\npath = \"../sibling_bin.rs\"\n[[test]]\nname = \"custom_test\"\npath = \"tests/custom_test.rs\"\n[[bench]]\nname = \"custom_bench\"\npath = \"benches/custom_bench.rs\"\nharness = false\n[[example]]\nname = \"custom_example\"\npath = \"examples/custom_example.rs\"\n",
        );
        write(&crate_dir.join("elsewhere/lib.rs"), "pub fn f() {}\n");
        write(&root.join("sibling_bin.rs"), "fn main() {}\n");
        write(
            &crate_dir.join("tests/custom_test.rs"),
            "#[test]\nfn t() {}\n",
        );
        write(&crate_dir.join("benches/custom_bench.rs"), "fn b() {}\n");
        write(
            &crate_dir.join("examples/custom_example.rs"),
            "fn main() {}\n",
        );

        for all_targets in [false, true] {
            let discovery = discover(dir.path(), all_targets);
            assert!(
                discovery.failures.is_empty(),
                "unexpected failures {:#?}",
                discovery.failures
            );
            assert_eq!(discovery.packages.len(), 1);
            let mut targets: Vec<(&str, bool, bool)> = discovery.packages[0]
                .targets
                .iter()
                .map(|target| (target.name.as_str(), target.doc, target.library))
                .collect();
            targets.sort_unstable();
            if all_targets {
                assert_eq!(
                    targets,
                    [
                        ("custom_bench", false, false),
                        ("custom_example", true, false),
                        ("custom_lib", true, true),
                        ("custom_test", false, false),
                        ("sibling", true, false),
                    ]
                );
            } else {
                assert_eq!(
                    targets,
                    [
                        ("custom_bench", false, false),
                        ("custom_example", false, false),
                        ("custom_lib", true, true),
                        ("custom_test", false, false),
                        ("sibling", false, false),
                    ]
                );
            }
            let sibling = discovery.packages[0]
                .targets
                .iter()
                .find(|target| target.name == "sibling")
                .unwrap();
            assert_eq!(sibling.src, root.join("sibling_bin.rs"));
        }
    }

    #[test]
    fn same_named_packages_in_different_workspaces_are_distinct() {
        let dir = tempfile::tempdir().unwrap();
        let root = &dir.path().join(SNAPSHOT_TREE);
        write(
            &root.join("first/Cargo.toml"),
            "[workspace]\nmembers = [\"shared\"]\nresolver = \"2\"\n",
        );
        member(&root.join("first/shared"), "shared");
        write(
            &root.join("second/Cargo.toml"),
            "[workspace]\nmembers = [\"shared\"]\nresolver = \"2\"\n",
        );
        member(&root.join("second/shared"), "shared");

        let discovery = discover(dir.path(), false);
        assert!(
            discovery.failures.is_empty(),
            "unexpected failures {:#?}",
            discovery.failures
        );
        assert_eq!(discovery.packages.len(), 2);
        for package in &discovery.packages {
            assert_eq!(package.name, "shared");
        }
        let mut manifests: Vec<&Path> = discovery
            .packages
            .iter()
            .map(|package| package.manifest.as_path())
            .collect();
        manifests.sort_unstable();
        assert_eq!(manifests[0], root.join("first/shared/Cargo.toml").as_path());
        assert_eq!(
            manifests[1],
            root.join("second/shared/Cargo.toml").as_path()
        );
    }

    #[test]
    fn hostile_snapshot_cargo_and_toolchain_configuration_has_no_effect() {
        let dir = tempfile::tempdir().unwrap();
        let root = &dir.path().join(SNAPSHOT_TREE);
        write(
            &root.join(".cargo/config.toml"),
            "[build]\nrustflags = [\"-Zunstable-flag\"]\n[target.x86_64-unknown-linux-gnu]\nlinker = \"/nonexistent/linker\"\n",
        );
        write(
            &root.join("rust-toolchain.toml"),
            "[toolchain]\nchannel = \"definitely-not-a-real-channel-99.99\"\n",
        );
        standalone(root, "hostile");

        let discovery = discover(dir.path(), false);
        assert!(
            discovery.failures.is_empty(),
            "unexpected failures {:#?}",
            discovery.failures
        );
        assert_eq!(discovery.packages.len(), 1);
        assert_eq!(discovery.packages[0].name, "hostile");
        for entry in std::fs::read_dir(root).unwrap() {
            assert_ne!(entry.unwrap().file_name(), "Cargo.lock");
        }
    }

    #[test]
    fn a_workspace_above_the_snapshot_is_never_consulted() {
        let dir = tempfile::tempdir().unwrap();
        let fixture = dir.path();
        let outside = fixture.join("outside");
        let commit = outside.join("commit");
        let snapshot = commit.join(SNAPSHOT_TREE);
        write(
            &outside.join("Cargo.toml"),
            "[workspace]\nmembers = [\"commit/tree\", \"other\"]\nresolver = \"2\"\n",
        );
        write(
            &outside.join("other/Cargo.toml"),
            "[package]\nname = \"other\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        );
        write(&outside.join("other/src/lib.rs"), "pub fn g() {}\n");
        write(
            &snapshot.join("Cargo.toml"),
            "[package]\nname = \"snappkg\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        );
        write(&snapshot.join("src/lib.rs"), "pub fn f() {}\n");
        let discovery = discover(&commit, false);
        assert_eq!(discovery.failures, Vec::<String>::new());
        let names: Vec<&str> = discovery
            .packages
            .iter()
            .map(|package| package.name.as_str())
            .collect();
        assert_eq!(names, ["snappkg"]);

        let commit = fixture.join("commit2");
        let snapshot = commit.join(SNAPSHOT_TREE);
        write(&commit.join("beyond.rs"), "fn main() {}\n");
        write(
            &snapshot.join("crate/Cargo.toml"),
            "[package]\nname = \"escaper\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[workspace]\n[[bin]]\nname = \"beyond\"\npath = \"../../beyond.rs\"\n",
        );
        write(&snapshot.join("crate/src/lib.rs"), "pub fn f() {}\n");
        let discovery = discover(&commit, false);
        assert_eq!(discovery.packages.len(), 1);
        let targets: Vec<&str> = discovery.packages[0]
            .targets
            .iter()
            .map(|target| target.name.as_str())
            .collect();
        assert_eq!(targets, ["escaper"]);
        assert_eq!(discovery.failures.len(), 1);
        assert!(
            discovery.failures[0].starts_with("crate/Cargo.toml: "),
            "unexpected failure {:?}",
            discovery.failures[0]
        );
        assert!(
            discovery.failures[0].contains("beyond"),
            "unexpected failure {:?}",
            discovery.failures[0]
        );
    }

    #[test]
    fn unresolved_manifests_are_reported_without_dropping_their_siblings() {
        let dir = tempfile::tempdir().unwrap();
        let root = &dir.path().join(SNAPSHOT_TREE);
        write(&root.join("bad/Cargo.toml"), "this is [not toml\n");
        write(
            &root.join("noname/Cargo.toml"),
            "[package]\nversion = \"0.1.0\"\nedition = \"2021\"\n[workspace]\n",
        );
        write(&root.join("noname/src/lib.rs"), "pub fn f() {}\n");
        write(
            &root.join("notargets/Cargo.toml"),
            "[package]\nname = \"notargets\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[workspace]\n",
        );
        standalone(&root.join("good"), "good");

        let discovery = discover(dir.path(), false);
        let names: Vec<&str> = discovery
            .packages
            .iter()
            .map(|package| package.name.as_str())
            .collect();
        assert_eq!(names, ["good"]);
        assert_eq!(discovery.failures.len(), 3);
        for anchor in [
            "bad/Cargo.toml",
            "noname/Cargo.toml",
            "notargets/Cargo.toml",
        ] {
            let expected = format!("{anchor}: ");
            assert!(
                discovery
                    .failures
                    .iter()
                    .any(|failure| failure.starts_with(&expected)),
                "no failure anchored at {anchor} in {:#?}",
                discovery.failures
            );
        }
        assert!(
            discovery
                .failures
                .iter()
                .any(|failure| failure.starts_with("noname/Cargo.toml: ")
                    && failure.contains("package.name")),
            "cargo's diagnosis is kept in {:#?}",
            discovery.failures
        );
    }

    #[test]
    fn an_empty_snapshot_discovers_nothing_without_failures() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join(SNAPSHOT_TREE);
        write(&root.join("README.md"), "# not a cargo project\n");
        let discovery = discover(dir.path(), false);
        assert_eq!(discovery.packages.len(), 0);
        assert_eq!(discovery.failures, Vec::<String>::new());
    }

    #[test]
    fn hidden_and_build_output_directories_are_searched_for_manifests() {
        let dir = tempfile::tempdir().unwrap();
        let root = &dir.path().join(SNAPSHOT_TREE);
        write(
            &root.join("target/fixture/Cargo.toml"),
            "[package]\nname = \"built\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[workspace]\n",
        );
        write(&root.join("target/fixture/src/lib.rs"), "pub fn f() {}\n");
        write(
            &root.join(".hidden/pkg/Cargo.toml"),
            "[package]\nname = \"hidden\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[workspace]\n",
        );
        write(&root.join(".hidden/pkg/src/lib.rs"), "pub fn f() {}\n");

        let discovery = discover(dir.path(), false);
        assert!(
            discovery.failures.is_empty(),
            "unexpected failures {:#?}",
            discovery.failures
        );
        let mut names: Vec<&str> = discovery
            .packages
            .iter()
            .map(|package| package.name.as_str())
            .collect();
        names.sort_unstable();
        assert_eq!(names, ["built", "hidden"]);
    }

    #[test]
    fn directory_aliases_are_discovered_once_and_file_aliases_follow_cargo() {
        let dir = tempfile::tempdir().unwrap();
        let root = &dir.path().join(SNAPSHOT_TREE);
        standalone(&root.join("pkg"), "aliased");
        std::fs::create_dir_all(root.join("alias")).unwrap();
        std::os::unix::fs::symlink("../pkg/Cargo.toml", root.join("alias/Cargo.toml")).unwrap();
        std::os::unix::fs::symlink("pkg", root.join("dupe")).unwrap();

        let discovery = discover(dir.path(), false);
        assert_eq!(discovery.packages.len(), 1);
        assert_eq!(discovery.packages[0].name, "aliased");
        assert!(discovery.packages[0].manifest.ends_with("pkg/Cargo.toml"));
        assert_eq!(discovery.failures.len(), 1, "{:?}", discovery.failures);
        assert!(discovery.failures[0].starts_with("alias/Cargo.toml: "));
    }

    #[test]
    fn link_cycles_and_ancestor_links_terminate_without_failures() {
        let dir = tempfile::tempdir().unwrap();
        let root = &dir.path().join(SNAPSHOT_TREE);
        standalone(&root.join("pkg"), "cycled");
        std::fs::create_dir_all(root.join("x")).unwrap();
        std::fs::create_dir_all(root.join("y")).unwrap();
        std::os::unix::fs::symlink("../y", root.join("x/link_y")).unwrap();
        std::os::unix::fs::symlink("../x", root.join("y/link_x")).unwrap();
        std::os::unix::fs::symlink(".", root.join("self")).unwrap();
        std::fs::create_dir_all(root.join("docs")).unwrap();
        std::os::unix::fs::symlink("../pkg/src/lib.rs", root.join("docs/lib.rs")).unwrap();

        let discovery = discover(dir.path(), false);
        assert!(
            discovery.failures.is_empty(),
            "unexpected failures {:#?}",
            discovery.failures
        );
        assert_eq!(discovery.packages.len(), 1);
        assert_eq!(discovery.packages[0].name, "cycled");
    }

    #[test]
    fn bad_links_and_special_files_fail_beside_a_healthy_package() {
        let dir = tempfile::tempdir().unwrap();
        let fixture = dir.path();
        let root = fixture.join(SNAPSHOT_TREE);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(fixture.join("outside")).unwrap();
        standalone(&root.join("pkg"), "healthy");
        std::os::unix::fs::symlink("does-not-exist", root.join("broken")).unwrap();
        std::os::unix::fs::symlink("../outside", root.join("escape")).unwrap();
        std::process::Command::new("mkfifo")
            .arg(root.join("pipe"))
            .status()
            .unwrap();

        let discovery = discover(fixture, false);
        let names: Vec<&str> = discovery
            .packages
            .iter()
            .map(|package| package.name.as_str())
            .collect();
        assert_eq!(names, ["healthy"]);
        assert_eq!(discovery.failures.len(), 3);
        for anchor in ["broken", "escape", "pipe"] {
            let expected = format!("{anchor}: ");
            assert!(
                discovery
                    .failures
                    .iter()
                    .any(|failure| failure.starts_with(&expected)),
                "no failure anchored at {anchor} in {:#?}",
                discovery.failures
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn shared_manifest_templates_keep_each_package_directory() {
        let dir = tempfile::tempdir().unwrap();
        let root = &dir.path().join(SNAPSHOT_TREE);
        let template = root.join("Cargo.template.toml");
        write(
            &template,
            "[package]\nname = \"shared\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[workspace]\n",
        );
        for name in ["alpha", "beta"] {
            let package = root.join(name);
            write(&package.join("src/lib.rs"), "pub fn f() {}\n");
            std::os::unix::fs::symlink("../Cargo.template.toml", package.join("Cargo.toml"))
                .unwrap();
        }
        let discovery = discover(dir.path(), false);
        assert!(discovery.failures.is_empty(), "{:?}", discovery.failures);
        let mut manifests: Vec<_> = discovery
            .packages
            .iter()
            .map(|package| package.manifest.strip_prefix(root).unwrap())
            .collect();
        manifests.sort_unstable();
        assert_eq!(
            manifests,
            [Path::new("alpha/Cargo.toml"), Path::new("beta/Cargo.toml")]
        );
        let mut sources: Vec<_> = discovery
            .packages
            .iter()
            .flat_map(|package| &package.targets)
            .filter(|target| target.library)
            .map(|target| target.src.strip_prefix(root).unwrap())
            .collect();
        sources.sort_unstable();
        assert_eq!(
            sources,
            [Path::new("alpha/src/lib.rs"), Path::new("beta/src/lib.rs")]
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_member_manifest_linked_outside_the_snapshot_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let root = &dir.path().join(SNAPSHOT_TREE);
        write(
            &root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"member\"]\nresolver = \"2\"\n",
        );
        write(&root.join("member/src/lib.rs"), "pub fn f() {}\n");
        let outside = dir.path().join("outside.toml");
        write(
            &outside,
            "[package]\nname = \"smuggled\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        );
        std::os::unix::fs::symlink(&outside, root.join("member/Cargo.toml")).unwrap();
        let discovery = discover(dir.path(), false);
        assert_eq!(discovery.packages.len(), 0);
        assert!(
            discovery
                .failures
                .iter()
                .any(|failure| failure.starts_with("Cargo.toml: ")
                    && failure.contains("outside the snapshot")),
            "{:#?}",
            discovery.failures
        );
    }
}
