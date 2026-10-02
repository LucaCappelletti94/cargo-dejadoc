//! Workspace and module-tree discovery.

use alloc::collections::BTreeSet;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;
use std::eprintln;
use std::format;
use std::path::PathBuf;

/// One compilable target to scan.
#[derive(Debug, Clone)]
pub(crate) struct Target {
    /// Crate name used as the item-path root.
    pub(crate) name: String,
    /// Absolute path of the target's source root file.
    pub(crate) src: PathBuf,
}

/// A resolved workspace root and the targets to scan.
#[derive(Debug, Clone)]
pub(crate) struct Workspace {
    /// Workspace root directory.
    pub(crate) root: PathBuf,
    /// Targets to scan, in metadata order.
    pub(crate) targets: Vec<Target>,
}

/// Resolve the workspace containing `root` and the targets to scan.
///
/// # Errors
///
/// Fails when `cargo metadata` cannot be run for `root`.
pub(crate) fn workspace(
    root: &std::path::Path,
    package: Option<&str>,
    all_targets: bool,
) -> crate::Result<Workspace> {
    let metadata = cargo_metadata::MetadataCommand::new()
        .current_dir(root)
        .no_deps()
        .exec()?;
    let mut targets = Vec::new();
    for package in metadata
        .packages
        .iter()
        .filter(|p| package.is_none_or(|want| p.name == want))
    {
        for target in &package.targets {
            if scan_target(&target.kind, all_targets) {
                targets.push(Target {
                    name: target.name.clone(),
                    src: target.src_path.clone().into(),
                });
            }
        }
    }
    Ok(Workspace {
        root: metadata.workspace_root.clone().into(),
        targets,
    })
}

/// Whether a cargo target is scanned, from its kind list.
fn scan_target(kinds: &[cargo_metadata::TargetKind], all_targets: bool) -> bool {
    use cargo_metadata::TargetKind as CargoTargetKind;

    if kinds
        .iter()
        .any(|k| matches!(k, CargoTargetKind::Lib | CargoTargetKind::ProcMacro))
    {
        return true;
    }
    all_targets
        && kinds
            .iter()
            .any(|k| matches!(k, CargoTargetKind::Bin | CargoTargetKind::Example))
}

/// Parse the target's module tree, each file with its module-path
/// segments from the target root (empty for the root file).
///
/// # Errors
///
/// Fails when a module file cannot be canonicalized. Unreadable or
/// unparseable files are skipped with a warning.
pub(crate) fn module_tree(target: &Target) -> crate::Result<Vec<crate::SourceFile>> {
    let mut out = Vec::new();
    let mut visited = BTreeSet::new();
    collect(&target.src, &[], true, &mut out, &mut visited)?;
    Ok(out)
}

/// `mod_rs` marks a crate root, a `mod.rs` or a `#[path]` file, whose children sit
/// beside it, every other file keeps them under a directory named after it.
fn collect(
    file: &std::path::Path,
    prefix: &[String],
    mod_rs: bool,
    out: &mut Vec<crate::SourceFile>,
    visited: &mut BTreeSet<PathBuf>,
) -> crate::Result<()> {
    let canonical = std::fs::canonicalize(file)?;
    if !visited.insert(canonical) {
        return Ok(());
    }
    let text = match std::fs::read_to_string(file) {
        Ok(text) => text,
        Err(err) => {
            eprintln!("dejadoc: cannot read {}: {err}", file.display());
            return Ok(());
        }
    };
    let parsed = match syn::parse_file(&text) {
        Ok(parsed) => parsed,
        Err(err) => {
            eprintln!("dejadoc: cannot parse {}: {err}", file.display());
            return Ok(());
        }
    };
    let dir = file.parent().unwrap_or_else(|| std::path::Path::new("."));
    let base = match file.file_stem() {
        Some(stem) if !mod_rs => dir.join(stem),
        _ => dir.to_path_buf(),
    };
    let mut children = Vec::new();
    mod_decls(&parsed.items, dir, &base, prefix, &mut children);
    out.push(crate::SourceFile {
        path: file.to_string_lossy().into_owned(),
        segments: prefix.to_vec(),
        parsed,
    });
    for (child, sub, mod_rs) in children {
        collect(&child, &sub, mod_rs, out, visited)?;
    }
    Ok(())
}

/// The `mod name;` files at every nesting depth whose `cfg` holds, each with its module
/// path. `dir` is for explicit `#[path]`, `base` for implicit children, an inline module
/// adds its name to both and to the path.
fn mod_decls(
    items: &[syn::Item],
    dir: &std::path::Path,
    base: &std::path::Path,
    prefix: &[String],
    out: &mut Vec<(PathBuf, Vec<String>, bool)>,
) {
    use syn::{Item, ext::IdentExt};

    for item in items {
        let Item::Mod(moditem) = item else {
            continue;
        };
        if !crate::cfg::allows(&moditem.attrs) {
            continue;
        }
        // A raw identifier names its file without the `r#`.
        let name = moditem.ident.unraw().to_string();
        let mut segments = prefix.to_vec();
        if let Some((_, children)) = &moditem.content {
            let inner = base.join(&name);
            segments.push(name);
            mod_decls(children, &inner, &inner, &segments, out);
            continue;
        }
        match resolve_mod_path(dir, base, &name, &moditem.attrs) {
            Some((path, mod_rs)) if path.exists() => {
                segments.push(name);
                out.push((path, segments, mod_rs));
            }
            Some((path, _)) => eprintln!("dejadoc: missing module file {}", path.display()),
            None => {}
        }
    }
}

/// File for `mod name;` and whether it is a mod-rs file. A top-level `#[path]` first, then
/// the first `cfg_attr` path whose predicate holds, both against `dir`, then `name.rs` or
/// `name/mod.rs` under `base`.
fn resolve_mod_path(
    dir: &std::path::Path,
    base: &std::path::Path,
    name: &str,
    attrs: &[syn::Attribute],
) -> Option<(PathBuf, bool)> {
    use syn::Meta;

    if let Some(attr) = attrs.iter().find(|a| a.path().is_ident("path")) {
        let Meta::NameValue(nv) = &attr.meta else {
            return None;
        };
        return Some((dir.join(crate::cfg::lit_str(&nv.value)?.value()), true));
    }
    let cfg_attr_path = attrs.iter().find_map(|attr| {
        crate::cfg::cfg_attr_metas(attr)?.find_map(|meta| match meta {
            Meta::NameValue(nv) if nv.path.is_ident("path") => {
                crate::cfg::lit_str(&nv.value).map(syn::LitStr::value)
            }
            _ => None,
        })
    });
    if let Some(path) = cfg_attr_path {
        return Some((dir.join(path), true));
    }
    let plain = base.join(format!("{name}.rs"));
    if plain.exists() {
        return Some((plain, false));
    }
    Some((base.join(name).join("mod.rs"), true))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(src: &std::path::Path) -> Target {
        Target {
            name: "mycrate".into(),
            src: src.to_path_buf(),
        }
    }

    /// Write `files` into a tempdir, the first being the crate root, and
    /// walk its module tree, each file as its tempdir-relative path and
    /// its module path.
    fn walk(files: &[(&str, &str)]) -> Vec<(String, String)> {
        let dir = tempfile::tempdir().unwrap();
        for (path, text) in files {
            let path = dir.path().join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        module_tree(&target(&dir.path().join(files[0].0)))
            .unwrap()
            .into_iter()
            .map(|file| {
                let path = std::path::Path::new(&file.path)
                    .strip_prefix(dir.path())
                    .unwrap();
                (
                    path.to_string_lossy().into_owned(),
                    file.segments.join("::"),
                )
            })
            .collect()
    }

    /// The tempdir-relative paths `walk` reaches.
    fn paths(files: &[(&str, &str)]) -> Vec<String> {
        walk(files).into_iter().map(|(path, _)| path).collect()
    }

    #[test]
    fn walks_declared_mod_files() {
        let got = paths(&[
            ("lib.rs", "pub mod a;\npub fn f() {}\n"),
            ("a.rs", "pub fn g() {}\n"),
        ]);
        assert_eq!(got, ["lib.rs", "a.rs"]);
    }

    #[test]
    fn resolves_mod_dir_form() {
        let got = paths(&[("lib.rs", "pub mod a;\n"), ("a/mod.rs", "pub fn g() {}\n")]);
        assert_eq!(got, ["lib.rs", "a/mod.rs"]);
    }

    #[test]
    fn cfg_attr_path_resolves_when_its_predicate_holds() {
        let got = paths(&[
            (
                "lib.rs",
                "#[cfg_attr(unix, path = \"other.rs\")]\npub mod renamed;\n",
            ),
            ("other.rs", "pub fn g() {}\n"),
        ]);
        assert_eq!(got, ["lib.rs", "other.rs"]);
    }

    #[test]
    fn cfg_attr_path_takes_the_holding_predicate_not_the_first_file() {
        let got = paths(&[
            (
                "lib.rs",
                "#[cfg_attr(windows, path = \"windows.rs\")]\n\
                 #[cfg_attr(unix, path = \"unix.rs\")]\n\
                 pub mod platform;\n",
            ),
            ("windows.rs", "pub fn w() {}\n"),
            ("unix.rs", "pub fn u() {}\n"),
        ]);
        assert_eq!(got, ["lib.rs", "unix.rs"]);
    }

    #[test]
    fn cfg_attr_path_with_a_failing_predicate_falls_back_to_the_plain_file() {
        let got = paths(&[
            (
                "lib.rs",
                "#[cfg_attr(windows, path = \"windows.rs\")]\npub mod platform;\n",
            ),
            ("windows.rs", "pub fn w() {}\n"),
            ("platform.rs", "pub fn p() {}\n"),
        ]);
        assert_eq!(got, ["lib.rs", "platform.rs"]);
    }

    #[test]
    fn cfg_attr_multi_token_predicate_resolves_path() {
        let got = paths(&[
            (
                "lib.rs",
                "#[cfg_attr(all(unix, not(windows)), path = \"other.rs\")]\npub mod renamed;\n",
            ),
            ("other.rs", "pub fn g() {}\n"),
        ]);
        assert_eq!(got, ["lib.rs", "other.rs"]);
    }

    #[test]
    fn cfg_attr_path_beside_other_attributes_resolves() {
        let got = paths(&[
            (
                "lib.rs",
                "#[cfg_attr(unix, doc = \"d.rs\", path = \"other.rs\")]\npub mod renamed;\n",
            ),
            ("other.rs", "pub fn g() {}\n"),
        ]);
        assert_eq!(got, ["lib.rs", "other.rs"]);
    }

    #[test]
    fn file_module_companion_directory() {
        // `de.rs` with a companion `de/` keeps its child modules in `de/`,
        // as in serde_derive.
        let got = paths(&[
            ("lib.rs", "pub mod de;\n"),
            ("de.rs", "pub mod child;\n"),
            ("de/child.rs", "pub fn g() {}\n"),
        ]);
        assert_eq!(got, ["lib.rs", "de.rs", "de/child.rs"]);
    }

    #[test]
    fn path_attribute_overrides_name() {
        let got = paths(&[
            ("lib.rs", "#[path = \"other.rs\"]\npub mod renamed;\n"),
            ("other.rs", "pub fn g() {}\n"),
        ]);
        assert_eq!(got, ["lib.rs", "other.rs"]);
    }

    #[test]
    fn raw_identifier_module_file() {
        // `mod r#extern;` resolves to `extern.rs`, not `r#extern.rs`.
        let got = paths(&[
            ("lib.rs", "pub mod r#extern;\n"),
            ("extern.rs", "pub fn g() {}\n"),
        ]);
        assert_eq!(got, ["lib.rs", "extern.rs"]);
    }

    #[test]
    fn path_attribute_uses_the_defining_directory() {
        // `#[path]` inside a file module is relative to the directory
        // containing that file, not its companion directory.
        let got = paths(&[
            ("lib.rs", "pub mod a;\n"),
            ("a.rs", "#[path = \"a/b.rs\"]\nmod b;\n"),
            ("a/b.rs", "pub fn g() {}\n"),
        ]);
        assert_eq!(got, ["lib.rs", "a.rs", "a/b.rs"]);
    }

    #[test]
    fn module_tree_reports_module_path_segments() {
        // File modules carry their module path, including raw-ident names
        // and companion-directory nesting.
        let modules: Vec<String> = walk(&[
            ("lib.rs", "pub mod r#extern;\npub mod a;\n"),
            ("extern.rs", "pub fn g() {}\n"),
            ("a.rs", "pub mod b;\n"),
            ("a/b.rs", "pub fn h() {}\n"),
        ])
        .into_iter()
        .map(|(_, module)| module)
        .collect();
        assert_eq!(modules, ["", "extern", "a", "a::b"]);
    }

    #[test]
    fn missing_mod_is_skipped_without_error() {
        let got = paths(&[("lib.rs", "pub mod missing;\npub fn f() {}\n")]);
        assert_eq!(got, ["lib.rs"]);
    }

    #[test]
    fn cfg_test_mod_files_are_not_collected() {
        let got = paths(&[
            (
                "lib.rs",
                "#[cfg(test)]\nmod tests;\n#[cfg(test)]\nmod inner { mod deep; }\n#[cfg(not(test))]\npub mod kept;\n",
            ),
            ("tests.rs", "pub fn g() {}\n"),
            ("inner/deep.rs", "pub fn g() {}\n"),
            ("kept.rs", "pub fn g() {}\n"),
        ]);
        assert_eq!(got, ["lib.rs", "kept.rs"]);
    }

    #[test]
    fn mod_decl_inside_inline_mod_resolves_under_the_inline_directory() {
        // `lib.rs` with `mod a { mod c; }` loads `a/c.rs`, a stale `c.rs`
        // beside `lib.rs` is not part of the crate.
        let got = walk(&[
            ("lib.rs", "pub mod a { pub mod c; }\n"),
            ("a/c.rs", "pub fn g() {}\n"),
            ("c.rs", "pub fn stale() {}\n"),
        ]);
        assert_eq!(
            got,
            [
                ("lib.rs".to_string(), String::new()),
                ("a/c.rs".to_string(), "a::c".to_string())
            ]
        );
    }

    #[test]
    fn mod_decl_inside_inline_mod_of_a_file_module_nests_under_its_directory() {
        // `x.rs` with `mod a { mod c; }` loads `x/a/c.rs`.
        let got = paths(&[
            ("lib.rs", "pub mod x;\n"),
            ("x.rs", "pub mod a { pub mod c; }\n"),
            ("x/a/c.rs", "pub fn g() {}\n"),
        ]);
        assert_eq!(got, ["lib.rs", "x.rs", "x/a/c.rs"]);
    }

    #[test]
    fn nested_inline_mods_stack_their_directories() {
        let got = paths(&[
            ("lib.rs", "pub mod a { pub mod b { pub mod c; } }\n"),
            ("a/b/c.rs", "pub fn g() {}\n"),
        ]);
        assert_eq!(got, ["lib.rs", "a/b/c.rs"]);
    }

    #[test]
    fn path_attribute_inside_inline_mod_resolves_under_the_inline_directory() {
        let got = paths(&[
            (
                "lib.rs",
                "pub mod a { #[path = \"other.rs\"] pub mod c; }\n",
            ),
            ("a/other.rs", "pub fn g() {}\n"),
            ("other.rs", "pub fn stale() {}\n"),
        ]);
        assert_eq!(got, ["lib.rs", "a/other.rs"]);
    }

    #[test]
    fn root_with_a_directory_named_after_it_keeps_children_beside_it() {
        let got = paths(&[
            ("lib.rs", "pub mod foo;\npub mod lib { pub mod child; }\n"),
            ("foo.rs", ""),
            ("lib/child.rs", ""),
            ("lib/foo.rs", "pub fn stale() {}\n"),
        ]);
        assert_eq!(got, ["lib.rs", "foo.rs", "lib/child.rs"]);
    }

    #[test]
    fn path_loaded_file_keeps_children_beside_it() {
        let got = paths(&[
            ("lib.rs", "#[path = \"other.rs\"]\npub mod m;\n"),
            ("other.rs", "pub mod c;\n"),
            ("c.rs", ""),
            ("other/c.rs", "pub fn stale() {}\n"),
        ]);
        assert_eq!(got, ["lib.rs", "other.rs", "c.rs"]);
    }

    #[test]
    fn mod_rs_file_keeps_children_beside_it() {
        let got = paths(&[
            ("lib.rs", "pub mod a;\n"),
            ("a/mod.rs", "pub mod b;\n"),
            ("a/b.rs", ""),
            ("a/mod/b.rs", "pub fn stale() {}\n"),
        ]);
        assert_eq!(got, ["lib.rs", "a/mod.rs", "a/b.rs"]);
    }

    #[test]
    fn module_cycles_terminate() {
        let got = paths(&[
            ("lib.rs", "pub mod a;\n"),
            ("a.rs", "#[path = \"lib.rs\"]\npub mod back;\n"),
        ]);
        assert_eq!(got, ["lib.rs", "a.rs"]);
    }

    #[test]
    fn unparseable_file_is_skipped_without_error() {
        assert_eq!(
            paths(&[("lib.rs", "@@@ not rust @@ @\n")]),
            Vec::<String>::new()
        );
    }

    fn cargo_package(dir: &std::path::Path, name: &str, extra: &str) {
        let src = dir.join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(
            dir.join("Cargo.toml"),
            format!(
                "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n{extra}"
            ),
        )
        .unwrap();
        std::fs::write(src.join("lib.rs"), "pub fn f() {}\n").unwrap();
    }

    #[test]
    fn default_is_lib_targets_only() {
        let dir = tempfile::tempdir().unwrap();
        cargo_package(
            dir.path(),
            "ws",
            // [workspace] stops cargo adopting an ancestor manifest, so
            // the fixture resolves the same wherever the tempdir lives.
            "[[bin]]\nname = \"tool\"\npath = \"src/bin/tool.rs\"\n[workspace]\n",
        );
        let bin = dir.path().join("src").join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("tool.rs"), "fn main() {}\n").unwrap();
        let ws = workspace(dir.path(), None, false).unwrap();
        assert_eq!(ws.targets.len(), 1);
        assert_eq!(ws.targets[0].name, "ws");
        assert!(ws.targets[0].src.ends_with("src/lib.rs"));
        assert_eq!(ws.root, dir.path().to_path_buf());
    }

    #[test]
    fn all_targets_includes_bins_and_examples() {
        let dir = tempfile::tempdir().unwrap();
        cargo_package(
            dir.path(),
            "ws",
            "[[bin]]\nname = \"tool\"\npath = \"src/bin/tool.rs\"\n[workspace]\n",
        );
        let bin = dir.path().join("src").join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("tool.rs"), "fn main() {}\n").unwrap();
        let ex = dir.path().join("examples");
        std::fs::create_dir_all(&ex).unwrap();
        std::fs::write(ex.join("demo.rs"), "fn main() {}\n").unwrap();
        let ws = workspace(dir.path(), None, true).unwrap();
        let names: Vec<&str> = ws.targets.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"ws"));
        assert!(names.contains(&"tool"));
        assert!(names.contains(&"demo"));
    }

    #[test]
    fn package_filter_selects_one_member() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"alpha\", \"beta\"]\nresolver = \"2\"\n",
        )
        .unwrap();
        cargo_package(dir.path().join("alpha").as_path(), "alpha", "");
        cargo_package(dir.path().join("beta").as_path(), "beta", "");
        let ws = workspace(dir.path(), Some("beta"), false).unwrap();
        assert_eq!(ws.targets.len(), 1);
        assert_eq!(ws.targets[0].name, "beta");
        let ws = workspace(dir.path(), Some("nope"), false).unwrap();
        assert!(ws.targets.is_empty());
    }
}
