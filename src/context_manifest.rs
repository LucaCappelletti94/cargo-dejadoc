//! A temporary manifest workspace that preserves consumer manifests and `Cargo.lock`.

use alloc::borrow::ToOwned;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use std::path::{Path, PathBuf};

/// A workspace member, as discovered by the bounded `--no-deps` metadata run.
pub(crate) struct MemberInfo {
    /// Absolute original manifest path.
    pub(crate) manifest: PathBuf,
    /// All targets of the member, from the discovery run.
    pub(crate) targets: Vec<TargetInfo>,
}

/// One target of a member, from the discovery run.
pub(crate) struct TargetInfo {
    /// Target name.
    pub(crate) name: String,
    /// Cargo target kinds, e.g. `lib`, `bin`, `custom-build`.
    pub(crate) kind: Vec<String>,
    /// Resolved crate types of the target.
    pub(crate) crate_types: Vec<String>,
    /// Absolute original source path.
    pub(crate) src_path: PathBuf,
    /// Resolved edition of the target.
    pub(crate) edition: Option<String>,
    /// Features the target requires to build.
    pub(crate) required_features: Vec<String>,
}

/// RAII temporary manifest mirror of a workspace root.
pub(crate) struct ManifestWorkspace {
    /// RAII handle keeping the mirror directory alive.
    _dir: tempfile::TempDir,
    temp_root: PathBuf,
    original_root: PathBuf,
}

impl ManifestWorkspace {
    /// Build the manifest mirror under a fresh tempdir.
    pub(crate) fn create(root: &Path, members: &[MemberInfo]) -> std::io::Result<Self> {
        let original_root = canonical_or_same(root);
        let dir = tempfile::tempdir()?;
        let temp_root = canonical_or_same(dir.path());
        let canonical: BTreeSet<PathBuf> = members
            .iter()
            .map(|member| canonical_or_same(&member.manifest))
            .collect();
        let destinations: BTreeMap<PathBuf, PathBuf> = canonical
            .iter()
            .map(|manifest| {
                (
                    manifest.clone(),
                    mirror_destination(&temp_root, manifest, &original_root),
                )
            })
            .collect();
        let by_manifest: BTreeMap<PathBuf, &MemberInfo> = members
            .iter()
            .map(|member| (canonical_or_same(&member.manifest), member))
            .collect();
        let mut written = BTreeSet::new();
        let root_manifest = original_root.join("Cargo.toml");
        write_manifest(
            &temp_root,
            &root_manifest,
            &original_root,
            &destinations,
            &by_manifest,
            &mut written,
        )?;
        for manifest in &canonical {
            write_manifest(
                &temp_root,
                manifest,
                &original_root,
                &destinations,
                &by_manifest,
                &mut written,
            )?;
        }
        let lock = original_root.join("Cargo.lock");
        if let Ok(bytes) = std::fs::read(&lock) {
            std::fs::write(temp_root.join("Cargo.lock"), bytes)?;
        }
        Ok(Self {
            _dir: dir,
            temp_root,
            original_root,
        })
    }

    /// The mirrored root manifest, for `--manifest-path`.
    pub(crate) fn manifest_path(&self) -> PathBuf {
        self.temp_root.join("Cargo.toml")
    }

    /// The mirror and original roots, to map reported paths and ids back.
    pub(crate) fn remap(&self) -> (String, String) {
        (
            self.temp_root.to_string_lossy().into_owned(),
            self.original_root.to_string_lossy().into_owned(),
        )
    }
}

/// Parse one manifest into a `toml::Value`.
fn parse_manifest(path: &Path) -> std::io::Result<toml::Value> {
    let text = std::fs::read_to_string(path)?;
    toml::from_str(&text).map_err(|err| invalid_data(&format!("parse {}: {err}", path.display())))
}

/// Serialize a manifest value to text.
fn render(value: &toml::Value) -> std::io::Result<String> {
    toml::to_string(value).map_err(|err| invalid_data(&format!("render manifest: {err}")))
}

/// An `io::Error` with a fixed kind and a formatted message.
fn invalid_data(detail: &str) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        format!("manifest mirror: {detail}"),
    )
}

/// The canonical form of `path`, `path` itself when it cannot be resolved.
fn canonical_or_same(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// The mirrored location of an original manifest, inside the tempdir.
fn mirror_destination(temp_root: &Path, manifest: &Path, original_root: &Path) -> PathBuf {
    let ancestor = common_ancestor(manifest, original_root);
    let rel = manifest
        .strip_prefix(&ancestor)
        .unwrap_or(Path::new("Cargo.toml"));
    temp_root.join(rel)
}

/// The deepest directory prefix shared by two canonical paths.
fn common_ancestor(a: &Path, b: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for (x, y) in a.components().zip(b.components()) {
        if x != y {
            break;
        }
        out.push(x);
    }
    out
}

/// Write one manifest into the mirror, remapped and with re-declared targets.
fn write_manifest(
    temp_root: &Path,
    manifest: &Path,
    original_root: &Path,
    destinations: &BTreeMap<PathBuf, PathBuf>,
    by_manifest: &BTreeMap<PathBuf, &MemberInfo>,
    written: &mut BTreeSet<PathBuf>,
) -> std::io::Result<()> {
    let manifest = canonical_or_same(manifest);
    if !written.insert(manifest.clone()) {
        return Ok(());
    }
    let value: toml::Value = parse_manifest(&manifest)?;
    let member = by_manifest.get(&manifest).copied();
    let is_root = manifest.parent() == Some(original_root);
    let value = mirror_manifest(
        value,
        &manifest,
        original_root,
        temp_root,
        member,
        destinations,
        is_root,
    );
    let dest = mirror_destination(temp_root, &manifest, original_root);
    dest.parent().map_or(Ok(()), std::fs::create_dir_all)?;
    std::fs::write(&dest, render(&value)?)?;
    Ok(())
}

/// Remap one manifest for the mirror.
fn mirror_manifest(
    value: toml::Value,
    manifest: &Path,
    original_root: &Path,
    temp_root: &Path,
    member: Option<&MemberInfo>,
    destinations: &BTreeMap<PathBuf, PathBuf>,
    is_root: bool,
) -> toml::Value {
    let manifest_dir = manifest.parent().unwrap_or(manifest);
    let mut value = value;
    if is_root
        && let Some(workspace) = value
            .get_mut("workspace")
            .and_then(toml::Value::as_table_mut)
    {
        let members: Vec<toml::Value> = destinations
            .values()
            .map(|dest| {
                let dir = dest.parent().map_or_else(|| dest.clone(), PathBuf::from);
                toml::Value::String(dir.to_string_lossy().into_owned())
            })
            .collect();
        workspace.insert("members".to_owned(), toml::Value::Array(members));
        if let Some(deps) = workspace
            .get_mut("dependencies")
            .and_then(toml::Value::as_table_mut)
        {
            remap_dep_table(deps, manifest_dir, destinations);
        }
    }
    if let Some(package) = value.get_mut("package").and_then(toml::Value::as_table_mut)
        && let Some(pointer) = package.get("workspace").and_then(toml::Value::as_str)
    {
        let resolved = manifest_dir.join(pointer);
        let resolved = resolved.canonicalize().unwrap_or(resolved);
        if resolved == *original_root {
            package.insert(
                "workspace".to_owned(),
                toml::Value::String(temp_root.to_string_lossy().into_owned()),
            );
        }
    }
    for table in ["dependencies", "dev-dependencies", "build-dependencies"] {
        if let Some(deps) = value.get_mut(table).and_then(toml::Value::as_table_mut) {
            remap_dep_table(deps, manifest_dir, destinations);
        }
    }
    if let Some(targets) = value.get_mut("target").and_then(toml::Value::as_table_mut) {
        for (_key, table) in targets.iter_mut() {
            let Some(table) = table.as_table_mut() else {
                continue;
            };
            for kind in ["dependencies", "dev-dependencies", "build-dependencies"] {
                if let Some(deps) = table.get_mut(kind).and_then(toml::Value::as_table_mut) {
                    remap_dep_table(deps, manifest_dir, destinations);
                }
            }
        }
    }
    for table_name in ["patch", "replace"] {
        if let Some(table) = value
            .get_mut(table_name)
            .and_then(toml::Value::as_table_mut)
        {
            for (_key, entry) in table.iter_mut() {
                remap_patch_entry(entry, manifest_dir, destinations);
            }
        }
    }
    if let Some(member) = member {
        redeclare_targets(&mut value, member);
    }
    value
}

/// Remap each dependency's `path` to a mirrored or absolute original directory.
fn remap_dep_table(
    deps: &mut toml::Table,
    manifest_dir: &Path,
    destinations: &BTreeMap<PathBuf, PathBuf>,
) {
    for (_key, value) in deps.iter_mut() {
        let Some(table) = value.as_table_mut() else {
            continue;
        };
        let Some(path) = table.get("path").and_then(toml::Value::as_str) else {
            continue;
        };
        table.insert(
            "path".to_owned(),
            toml::Value::String(
                mirrored_dep_path(manifest_dir, path, destinations)
                    .to_string_lossy()
                    .into_owned(),
            ),
        );
    }
}

/// Remap a `path` field under `patch` or `replace`.
fn remap_patch_entry(
    entry: &mut toml::Value,
    manifest_dir: &Path,
    destinations: &BTreeMap<PathBuf, PathBuf>,
) {
    let Some(table) = entry.as_table_mut() else {
        return;
    };
    for (_key, value) in table.iter_mut() {
        let Some(deps) = value.as_table_mut() else {
            continue;
        };
        let Some(path) = deps.get("path").and_then(toml::Value::as_str) else {
            continue;
        };
        deps.insert(
            "path".to_owned(),
            toml::Value::String(
                mirrored_dep_path(manifest_dir, path, destinations)
                    .to_string_lossy()
                    .into_owned(),
            ),
        );
    }
}

/// The directory a path dependency resolves to, in the mirror or at the original.
fn mirrored_dep_path(
    manifest_dir: &Path,
    path: &str,
    destinations: &BTreeMap<PathBuf, PathBuf>,
) -> PathBuf {
    let resolved = manifest_dir.join(path);
    let resolved = resolved.canonicalize().unwrap_or(resolved);
    let manifest = resolved.join("Cargo.toml");
    let manifest = manifest.canonicalize().unwrap_or(manifest);
    match destinations.get(&manifest) {
        Some(dest) => dest.parent().map_or_else(|| dest.clone(), PathBuf::from),
        None => resolved,
    }
}

/// Re-declare every target of a member from the discovery run, and disable
/// auto-discovery, so the mirror needs no source files.
fn redeclare_targets(value: &mut toml::Value, member: &MemberInfo) {
    let table = value.as_table_mut().expect("a manifest root is a table");
    table.remove("lib");
    for key in ["bin", "bench", "test", "example"] {
        table.remove(key);
    }
    let Some(package) = table.get_mut("package").and_then(toml::Value::as_table_mut) else {
        return;
    };
    package.remove("build");
    for flag in ["autobins", "autoexamples", "autotests", "autobenches"] {
        package.insert(flag.to_owned(), toml::Value::Boolean(false));
    }
    if let Some(build) = member
        .targets
        .iter()
        .find(|target| target.kind.iter().any(|kind| kind == "custom-build"))
    {
        package.insert(
            "build".to_owned(),
            toml::Value::String(build.src_path.to_string_lossy().into_owned()),
        );
    }
    for target in &member.targets {
        if target.kind.iter().any(|kind| kind == "custom-build") {
            continue;
        }
        if target.kind.iter().any(|kind| {
            matches!(
                kind.as_str(),
                "lib" | "rlib" | "dylib" | "cdylib" | "staticlib" | "proc-macro"
            )
        }) {
            let mut lib = toml::Table::new();
            insert_target_fields(&mut lib, target);
            if target.kind.iter().any(|kind| kind == "proc-macro") {
                lib.insert("proc-macro".to_owned(), toml::Value::Boolean(true));
            } else if !target.crate_types.is_empty() && target.crate_types != ["lib"] {
                lib.insert(
                    "crate-type".to_owned(),
                    toml::Value::Array(
                        target
                            .crate_types
                            .iter()
                            .map(|kind| toml::Value::String(kind.clone()))
                            .collect(),
                    ),
                );
            }
            table.insert("lib".to_owned(), toml::Value::Table(lib));
            continue;
        }
        let Some(key) = target
            .kind
            .iter()
            .find(|kind| matches!(kind.as_str(), "bin" | "example" | "test" | "bench"))
            .cloned()
        else {
            continue;
        };
        let mut item = toml::Table::new();
        insert_target_fields(&mut item, target);
        let Some(items) = table.get_mut(&key).and_then(toml::Value::as_array_mut) else {
            table.insert(key.clone(), toml::Value::Array(Vec::new()));
            table
                .get_mut(&key)
                .and_then(toml::Value::as_array_mut)
                .unwrap()
                .push(toml::Value::Table(item));
            continue;
        };
        items.push(toml::Value::Table(item));
    }
}

/// The shared fields of one re-declared target.
fn insert_target_fields(map: &mut toml::Table, target: &TargetInfo) {
    map.insert("name".to_owned(), toml::Value::String(target.name.clone()));
    map.insert(
        "path".to_owned(),
        toml::Value::String(target.src_path.to_string_lossy().into_owned()),
    );
    if let Some(edition) = &target.edition {
        map.insert("edition".to_owned(), toml::Value::String(edition.clone()));
    }
    if !target.required_features.is_empty() {
        map.insert(
            "required-features".to_owned(),
            toml::Value::Array(
                target
                    .required_features
                    .iter()
                    .map(|feature| toml::Value::String(feature.clone()))
                    .collect(),
            ),
        );
    }
}

/// Map `value`'s mirror paths and ids back to the originals.
pub(crate) fn remap_value(
    value: serde_json::Value,
    temp_root: &str,
    original_root: &str,
) -> serde_json::Value {
    let mut value = value;
    let mut replace = |field: &str, remap: &dyn Fn(&str) -> String| {
        let current = value
            .get(field)
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned);
        let Some(current) = current else {
            return;
        };
        let remapped = remap(&current);
        if remapped != current {
            value[field] = serde_json::Value::String(remapped);
        }
    };
    replace("workspace_root", &|s| {
        remap_path_str(s, temp_root, original_root)
    });
    replace("target_directory", &|s| {
        remap_path_str(s, temp_root, original_root)
    });
    if let Some(members) = value
        .get_mut("workspace_members")
        .and_then(serde_json::Value::as_array_mut)
    {
        for member in members {
            let Some(current) = member.as_str() else {
                continue;
            };
            let remapped = remap_id_str(current, temp_root, original_root);
            if remapped != current {
                *member = serde_json::Value::String(remapped);
            }
        }
    }
    if let Some(packages) = value
        .get_mut("packages")
        .and_then(serde_json::Value::as_array_mut)
    {
        for package in packages {
            replace_package_field(package, "id", &|s| {
                remap_id_str(s, temp_root, original_root)
            });
            replace_package_field(package, "manifest_path", &|s| {
                remap_path_str(s, temp_root, original_root)
            });
        }
    }
    if let Some(nodes) = value
        .get_mut("resolve")
        .and_then(serde_json::Value::as_object_mut)
        .and_then(|resolve| resolve.get_mut("nodes"))
        .and_then(serde_json::Value::as_array_mut)
    {
        for node in nodes {
            replace_package_field(node, "id", &|s| remap_id_str(s, temp_root, original_root));
            if let Some(deps) = node
                .get_mut("deps")
                .and_then(serde_json::Value::as_array_mut)
            {
                for dep in deps {
                    replace_package_field(dep, "pkg", &|s| {
                        remap_id_str(s, temp_root, original_root)
                    });
                }
            }
        }
    }
    value
}

/// Replace one string field of an object, remapping a mirror prefix.
fn replace_package_field(
    object: &mut serde_json::Value,
    field: &str,
    remap: &dyn Fn(&str) -> String,
) {
    let current = object
        .get(field)
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    let Some(current) = current else {
        return;
    };
    let remapped = remap(&current);
    if remapped != current {
        object[field] = serde_json::Value::String(remapped);
    }
}

/// Replace a mirror path prefix with the original, keeping a valid path.
fn remap_path_str(s: &str, temp: &str, original: &str) -> String {
    if s == temp {
        return original.to_owned();
    }
    if let Some(rest) = s.strip_prefix(&format!("{temp}/")) {
        return format!("{original}/{rest}");
    }
    s.to_owned()
}

/// Replace a mirror prefix inside a package id, which embeds a `file://` path.
fn remap_id_str(s: &str, temp: &str, original: &str) -> String {
    match s.find(temp) {
        Some(idx) => {
            let after = idx + temp.len();
            if after == s.len() || s.as_bytes().get(after) == Some(&b'/') {
                s.replacen(temp, original, 1)
            } else {
                s.to_owned()
            }
        }
        None => s.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use std::fs;

    fn put(root: &Path, rel: &str, text: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn read(path: &Path) -> String {
        fs::read_to_string(path).unwrap()
    }

    fn member(manifest: PathBuf) -> MemberInfo {
        MemberInfo {
            manifest,
            targets: Vec::new(),
        }
    }

    fn temp_root(workspace: &ManifestWorkspace) -> String {
        workspace.remap().0
    }

    fn create_ok(root: &Path, m: &[MemberInfo]) -> ManifestWorkspace {
        ManifestWorkspace::create(root, m).unwrap_or_else(|e| panic!("create the mirror: {e}"))
    }

    fn create_broken(root: &Path, m: &[MemberInfo]) -> std::io::Error {
        ManifestWorkspace::create(root, m)
            .err()
            .expect("the manifest must fail")
    }

    #[test]
    fn create_rejects_a_broken_root_manifest() {
        let dir = tempfile::tempdir().unwrap();
        put(dir.path(), "Cargo.toml", "[package\nname = \"broken\"");
        let err = create_broken(dir.path(), &[]);
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        let message = format!("{err}");
        assert!(message.contains("manifest mirror"), "{message}");
        assert!(message.contains("parse"), "{message}");
    }

    #[test]
    fn create_rejects_a_broken_member_manifest() {
        let dir = tempfile::tempdir().unwrap();
        put(dir.path(), "Cargo.toml", "[workspace]\nmembers = [\"m\"]\n");
        put(dir.path(), "m/Cargo.toml", "[workspace\n");
        let members = vec![member(dir.path().join("m/Cargo.toml"))];
        let err = create_broken(dir.path(), &members);
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn create_skips_the_root_when_it_is_its_own_member() {
        let dir = tempfile::tempdir().unwrap();
        put(
            dir.path(),
            "Cargo.toml",
            "[workspace]\nmembers = [\".\"]\n\n[package]\nname = \"root\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        );
        let members = vec![member(dir.path().join("Cargo.toml"))];
        let workspace = create_ok(dir.path(), &members);
        let mirror = read(
            Path::new(temp_root(&workspace).as_str())
                .join("Cargo.toml")
                .as_path(),
        );
        assert!(mirror.contains("name = \"root\""), "{mirror}");
    }

    #[test]
    fn create_mirrors_a_member_outside_the_root() {
        let dir = tempfile::tempdir().unwrap();
        put(dir.path(), "Cargo.toml", "[workspace]\nmembers = []\n");
        let outside = tempfile::tempdir().unwrap();
        put(
            outside.path(),
            "Cargo.toml",
            "[package]\nname = \"out\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        );
        let members = vec![member(outside.path().join("Cargo.toml"))];
        let workspace = create_ok(dir.path(), &members);
        let mirror_dir =
            Path::new(temp_root(&workspace).as_str()).join(outside.path().file_name().unwrap());
        let mirror = read(mirror_dir.join("Cargo.toml").as_path());
        assert!(mirror.contains("name = \"out\""), "{mirror}");
    }

    #[test]
    fn create_repoints_a_member_workspace_pointer() {
        let dir = tempfile::tempdir().unwrap();
        put(dir.path(), "Cargo.toml", "[workspace]\nmembers = [\"m\"]\n");
        put(
            dir.path(),
            "m/Cargo.toml",
            "[package]\nname = \"m\"\nversion = \"0.1.0\"\nedition = \"2024\"\nworkspace = \"..\"\n",
        );
        let members = vec![member(dir.path().join("m/Cargo.toml"))];
        let workspace = create_ok(dir.path(), &members);
        let temp_root = temp_root(&workspace);
        let mirror = read(Path::new(temp_root.as_str()).join("m/Cargo.toml").as_path());
        assert!(
            mirror.contains(&format!("workspace = \"{temp_root}\"")),
            "{mirror}"
        );
    }

    #[test]
    fn create_remakes_target_scoped_and_odd_target_tables() {
        let dir = tempfile::tempdir().unwrap();
        put(
            dir.path(),
            "Cargo.toml",
            "[workspace]\nmembers = [\"a\", \"b\"]\n",
        );
        put(
            dir.path(),
            "a/Cargo.toml",
            "[package]\nname = \"a\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[target.'cfg(unix)'.dependencies]\nb = { path = \"../b\" }\n",
        );
        put(
            dir.path(),
            "b/Cargo.toml",
            "[target]\nodd = 1\n\n[package]\nname = \"b\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        );
        let members = vec![
            member(dir.path().join("a/Cargo.toml")),
            member(dir.path().join("b/Cargo.toml")),
        ];
        let workspace = create_ok(dir.path(), &members);
        let temp_root = temp_root(&workspace);
        let mirror = read(Path::new(temp_root.as_str()).join("a/Cargo.toml").as_path());
        let mirror_b = read(Path::new(temp_root.as_str()).join("b/Cargo.toml").as_path());
        assert!(
            mirror.contains(&format!("path = \"{temp_root}/b\"")),
            "{mirror}"
        );
        assert!(mirror_b.contains("odd = 1"), "{mirror_b}");
    }

    #[test]
    fn create_remakes_patch_and_replace_path_entries() {
        let dir = tempfile::tempdir().unwrap();
        put(
            dir.path(),
            "Cargo.toml",
            "[workspace]\nmembers = [\"a\", \"b\"]\n",
        );
        put(
            dir.path(),
            "a/Cargo.toml",
            "[package]\nname = \"a\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[dependencies]\nb = { path = \"../b\" }\n[patch.crates-io]\nb = { path = \"../b\" }\nstray = 1\n[replace.foo]\nb = { git = \"https://example.test/b\" }\n",
        );
        put(
            dir.path(),
            "b/Cargo.toml",
            "[package]\nname = \"b\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[replace]\nodd = 2\n",
        );
        let members = vec![
            member(dir.path().join("a/Cargo.toml")),
            member(dir.path().join("b/Cargo.toml")),
        ];
        let workspace = create_ok(dir.path(), &members);
        let temp_root = temp_root(&workspace);
        let mirror = read(Path::new(temp_root.as_str()).join("a/Cargo.toml").as_path());
        let mirror_b = read(Path::new(temp_root.as_str()).join("b/Cargo.toml").as_path());
        assert!(
            mirror.contains(&format!("path = \"{temp_root}/b\"")),
            "{mirror}"
        );
        assert!(mirror.contains("stray = 1"), "{mirror}");
        assert!(
            mirror.contains("git = \"https://example.test/b\""),
            "{mirror}"
        );
        assert!(mirror_b.contains("odd = 2"), "{mirror_b}");
    }

    #[test]
    fn create_keeps_version_and_git_dependencies_bare() {
        let dir = tempfile::tempdir().unwrap();
        put(dir.path(), "Cargo.toml", "[workspace]\nmembers = [\"m\"]\n");
        put(
            dir.path(),
            "m/Cargo.toml",
            "[package]\nname = \"m\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[dependencies]\nserde = \"1\"\nextra = { git = \"https://example.test/extra\" }\n",
        );
        let members = vec![member(dir.path().join("m/Cargo.toml"))];
        let workspace = create_ok(dir.path(), &members);
        let mirror = read(
            Path::new(temp_root(&workspace).as_str())
                .join("m/Cargo.toml")
                .as_path(),
        );
        assert!(mirror.contains("serde = \"1\""), "{mirror}");
        assert!(
            mirror.contains("git = \"https://example.test/extra\""),
            "{mirror}"
        );
    }

    #[test]
    fn create_declares_the_build_target_from_discovery() {
        let dir = tempfile::tempdir().unwrap();
        put(dir.path(), "Cargo.toml", "[workspace]\nmembers = [\"m\"]\n");
        put(
            dir.path(),
            "m/Cargo.toml",
            "[package]\nname = \"m\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        );
        let src = dir.path().join("m/build.rs");
        fs::write(&src, "// build\n").unwrap();
        let members = vec![MemberInfo {
            manifest: dir.path().join("m/Cargo.toml"),
            targets: vec![TargetInfo {
                name: "build".into(),
                kind: vec!["custom-build".into()],
                crate_types: Vec::new(),
                src_path: src.clone(),
                edition: None,
                required_features: Vec::new(),
            }],
        }];
        let workspace = create_ok(dir.path(), &members);
        let mirror = read(
            Path::new(temp_root(&workspace).as_str())
                .join("m/Cargo.toml")
                .as_path(),
        );
        assert!(
            mirror.contains(&format!("build = \"{}\"", src.display())),
            "{mirror}"
        );
    }

    #[test]
    fn create_declares_proc_macro_and_crate_type_targets() {
        let dir = tempfile::tempdir().unwrap();
        put(
            dir.path(),
            "Cargo.toml",
            "[workspace]\nmembers = [\"p\", \"c\"]\n",
        );
        put(
            dir.path(),
            "p/Cargo.toml",
            "[package]\nname = \"p\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        );
        put(
            dir.path(),
            "c/Cargo.toml",
            "[package]\nname = \"c\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        );
        let members = vec![
            MemberInfo {
                manifest: dir.path().join("p/Cargo.toml"),
                targets: vec![TargetInfo {
                    name: "p".into(),
                    kind: vec!["proc-macro".into()],
                    crate_types: Vec::new(),
                    src_path: dir.path().join("p/src/lib.rs"),
                    edition: None,
                    required_features: Vec::new(),
                }],
            },
            MemberInfo {
                manifest: dir.path().join("c/Cargo.toml"),
                targets: vec![TargetInfo {
                    name: "c".into(),
                    kind: vec!["lib".into()],
                    crate_types: vec!["lib".into(), "cdylib".into()],
                    src_path: dir.path().join("c/src/lib.rs"),
                    edition: None,
                    required_features: Vec::new(),
                }],
            },
        ];
        let workspace = create_ok(dir.path(), &members);
        let temp_root = temp_root(&workspace);
        let mirror = read(Path::new(temp_root.as_str()).join("p/Cargo.toml").as_path());
        assert!(mirror.contains("proc-macro = true"), "{mirror}");
        let mirror = read(Path::new(temp_root.as_str()).join("c/Cargo.toml").as_path());
        assert!(
            mirror.contains("crate-type = [\"lib\", \"cdylib\"]"),
            "{mirror}"
        );
    }

    #[test]
    fn create_skips_an_unrecognized_target_kind() {
        let dir = tempfile::tempdir().unwrap();
        put(dir.path(), "Cargo.toml", "[workspace]\nmembers = [\"m\"]\n");
        put(
            dir.path(),
            "m/Cargo.toml",
            "[package]\nname = \"m\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        );
        let members = vec![MemberInfo {
            manifest: dir.path().join("m/Cargo.toml"),
            targets: vec![TargetInfo {
                name: "weird".into(),
                kind: vec!["weird".into()],
                crate_types: Vec::new(),
                src_path: dir.path().join("m/weird.rs"),
                edition: None,
                required_features: Vec::new(),
            }],
        }];
        let workspace = create_ok(dir.path(), &members);
        let mirror = read(
            Path::new(temp_root(&workspace).as_str())
                .join("m/Cargo.toml")
                .as_path(),
        );
        assert!(!mirror.contains("[lib]"), "{mirror}");
    }

    #[test]
    fn create_appends_a_second_bin_with_required_features() {
        let dir = tempfile::tempdir().unwrap();
        put(dir.path(), "Cargo.toml", "[workspace]\nmembers = [\"m\"]\n");
        put(
            dir.path(),
            "m/Cargo.toml",
            "[package]\nname = \"m\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        );
        let members = vec![MemberInfo {
            manifest: dir.path().join("m/Cargo.toml"),
            targets: vec![
                TargetInfo {
                    name: "one".into(),
                    kind: vec!["bin".into()],
                    crate_types: vec!["bin".into()],
                    src_path: dir.path().join("m/src/bin/one.rs"),
                    edition: None,
                    required_features: Vec::new(),
                },
                TargetInfo {
                    name: "two".into(),
                    kind: vec!["bin".into()],
                    crate_types: vec!["bin".into()],
                    src_path: dir.path().join("m/src/bin/two.rs"),
                    edition: None,
                    required_features: vec!["f1".into()],
                },
            ],
        }];
        let workspace = create_ok(dir.path(), &members);
        let mirror = read(
            Path::new(temp_root(&workspace).as_str())
                .join("m/Cargo.toml")
                .as_path(),
        );
        assert_eq!(mirror.matches("[[bin]]").count(), 2, "{mirror}");
        assert!(mirror.contains("required-features = [\"f1\"]"), "{mirror}");
    }

    #[test]
    fn create_keeps_a_member_without_a_package_table() {
        let dir = tempfile::tempdir().unwrap();
        put(dir.path(), "Cargo.toml", "[workspace]\nmembers = [\"m\"]\n");
        put(dir.path(), "m/Cargo.toml", "[workspace]\nmembers = []\n");
        let members = vec![member(dir.path().join("m/Cargo.toml"))];
        let workspace = create_ok(dir.path(), &members);
        let mirror = read(
            Path::new(temp_root(&workspace).as_str())
                .join("m/Cargo.toml")
                .as_path(),
        );
        assert!(mirror.contains("[workspace]"), "{mirror}");
        assert!(!mirror.contains("autobins"), "{mirror}");
    }

    #[test]
    fn remap_value_keeps_unknown_and_sparse_documents() {
        let sparse = serde_json::json!({"workspace_members": [123]});
        assert_eq!(remap_value(sparse.clone(), "/tmp/a", "/orig"), sparse);
        let nodes = serde_json::json!({
            "packages": [{"name": "x"}],
            "resolve": {"nodes": [{"id": "path+dep"}]}
        });
        assert_eq!(remap_value(nodes.clone(), "/tmp/a", "/orig"), nodes);
    }

    #[test]
    fn remap_value_keeps_a_non_boundary_id_occurrence() {
        let value = serde_json::json!({"workspace_members": ["path+file:///tmp/ab/dep#sha"]});
        assert_eq!(remap_value(value.clone(), "/tmp/a", "/orig"), value);
    }
}
