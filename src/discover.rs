use std::collections::HashSet;
use std::path::PathBuf;

/// One compilable target to scan.
#[derive(Debug, Clone)]
pub struct Target {
    /// Crate name used as the item-path root.
    pub name: String,
    /// Target kind.
    pub kind: TargetKind,
    /// Absolute path of the target's source root file.
    pub src: PathBuf,
}

/// Kind of target, matching what `cargo test --doc` runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetKind {
    Lib,
    Bin,
    Example,
}

/// A resolved workspace: root directory and the targets to scan.
#[derive(Debug, Clone)]
pub struct Workspace {
    /// Workspace root directory.
    pub root: PathBuf,
    /// Targets to scan, in metadata order.
    pub targets: Vec<Target>,
}

/// Resolve the workspace containing `root` and the targets to scan.
pub fn workspace(
    root: &std::path::Path,
    package: Option<&str>,
    all_targets: bool,
) -> anyhow::Result<Workspace> {
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
        for target in package.targets.iter() {
            if let Some(kind) = scan_kind(&target.kind, all_targets) {
                targets.push(Target {
                    name: target.name.clone(),
                    kind,
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

/// Scan kind of a cargo target from its kind list.
fn scan_kind(kinds: &[cargo_metadata::TargetKind], all_targets: bool) -> Option<TargetKind> {
    use cargo_metadata::TargetKind as CargoTargetKind;

    if kinds
        .iter()
        .any(|k| matches!(k, CargoTargetKind::Lib | CargoTargetKind::ProcMacro))
    {
        return Some(TargetKind::Lib);
    }
    if !all_targets {
        return None;
    }
    if kinds.iter().any(|k| matches!(k, CargoTargetKind::Bin)) {
        return Some(TargetKind::Bin);
    }
    if kinds.iter().any(|k| matches!(k, CargoTargetKind::Example)) {
        return Some(TargetKind::Example);
    }
    None
}

/// Walk the module tree from a target root: the root file plus every
/// `mod`-declared file, with `syn` parse results.
pub fn module_tree(target: &Target) -> anyhow::Result<Vec<(PathBuf, syn::File)>> {
    let mut out = Vec::new();
    let mut visited = HashSet::new();
    collect(&target.src, &mut out, &mut visited)?;
    Ok(out)
}

fn collect(
    file: &std::path::Path,
    out: &mut Vec<(PathBuf, syn::File)>,
    visited: &mut HashSet<PathBuf>,
) -> anyhow::Result<()> {
    let canonical = std::fs::canonicalize(file)?;
    if !visited.insert(canonical) {
        return Ok(());
    }
    let text = match std::fs::read_to_string(file) {
        Ok(text) => text,
        Err(err) => {
            eprintln!("dejadoc: warning: cannot read {file:?}: {err}");
            return Ok(());
        }
    };
    let parsed = match syn::parse_file(&text) {
        Ok(parsed) => parsed,
        Err(err) => {
            eprintln!("dejadoc: warning: cannot parse {file:?}: {err}");
            return Ok(());
        }
    };
    let dir = file.parent().unwrap_or_else(|| std::path::Path::new("."));
    // A file module with a companion directory keeps its child modules
    // there; `#[path]` in that file is still relative to `dir`.
    let base = match file.file_stem() {
        Some(stem) if dir.join(stem).is_dir() => dir.join(stem),
        _ => dir.to_path_buf(),
    };
    let mut children = Vec::new();
    mod_decls(&parsed.items, dir, &base, &mut children);
    out.push((file.to_path_buf(), parsed));
    for child in children {
        collect(&child, out, visited)?;
    }
    Ok(())
}

/// Collect the files of `mod name;` declarations, at every nesting depth.
/// `dir` is the directory containing the defining file (explicit `#[path]`
/// resolves against it); `base` is the implicit child directory (the
/// companion directory for a file module).
fn mod_decls(
    items: &[syn::Item],
    dir: &std::path::Path,
    base: &std::path::Path,
    out: &mut Vec<PathBuf>,
) {
    use syn::Item;

    for item in items {
        if let Item::Mod(moditem) = item {
            if let Some((_, children)) = &moditem.content {
                mod_decls(children, dir, base, out);
                continue;
            }
            match resolve_mod_path(dir, base, &moditem.ident, &moditem.attrs) {
                Some(path) if path.exists() => out.push(path),
                Some(path) => eprintln!("dejadoc: warning: missing module file {path:?}"),
                None => {}
            }
        }
    }
}

/// File for `mod name;` in `dir`. A top-level `#[path = "…"]` wins, as it
/// does for rustc; otherwise the first existing `path` from a
/// `#[cfg_attr(…, path = "…")]` is used, since cfg is not evaluated.
/// Explicit paths resolve against `dir`; the plain fallback against `base`.
fn resolve_mod_path(
    dir: &std::path::Path,
    base: &std::path::Path,
    name: &syn::Ident,
    attrs: &[syn::Attribute],
) -> Option<PathBuf> {
    use syn::{Expr, ExprLit, Lit, Meta};

    let name = name.to_string().trim_start_matches("r#").to_string();
    if let Some(attr) = attrs.iter().find(|a| a.path().is_ident("path")) {
        let Meta::NameValue(nv) = &attr.meta else {
            return None;
        };
        let Expr::Lit(ExprLit {
            lit: Lit::Str(s), ..
        }) = &nv.value
        else {
            return None;
        };
        return Some(dir.join(s.value()));
    }
    let candidates: Vec<PathBuf> = attrs
        .iter()
        .filter(|a| a.path().is_ident("cfg_attr"))
        .filter_map(|a| {
            let Meta::List(list) = &a.meta else {
                return None;
            };
            Some(cfg_attr_paths(&list.tokens))
        })
        .flatten()
        .map(|p| dir.join(p))
        .collect();
    if let Some(path) = candidates.into_iter().find(|p| p.exists()) {
        return Some(path);
    }
    let plain = base.join(format!("{name}.rs"));
    if plain.exists() {
        return Some(plain);
    }
    Some(base.join(&name).join("mod.rs"))
}

/// The `path = "…"` values inside a `cfg_attr` attribute's tokens.
fn cfg_attr_paths(tokens: &proc_macro2::TokenStream) -> Vec<String> {
    let trees = tokens.clone().into_iter().collect::<Vec<_>>();
    let mut out = Vec::new();
    let mut i = 0;
    while i + 2 < trees.len() {
        if matches!(
            (&trees[i], &trees[i + 1]),
            (
                proc_macro2::TokenTree::Ident(id),
                proc_macro2::TokenTree::Punct(eq)
            ) if id == "path" && eq.as_char() == '='
        ) {
            let text = trees[i + 2].to_string();
            if let Ok(s) = syn::parse_str::<syn::LitStr>(&text) {
                out.push(s.value());
            }
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(src: &std::path::Path) -> Target {
        Target {
            name: "mycrate".into(),
            kind: TargetKind::Lib,
            src: src.to_path_buf(),
        }
    }

    fn files(result: Vec<(PathBuf, syn::File)>) -> Vec<PathBuf> {
        result.into_iter().map(|(p, _)| p).collect()
    }

    #[test]
    fn walks_declared_mod_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("lib.rs");
        std::fs::write(&root, "pub mod a;\npub fn f() {}\n").unwrap();
        std::fs::write(dir.path().join("a.rs"), "pub fn g() {}\n").unwrap();
        let result = module_tree(&target(&root)).unwrap();
        let got: Vec<String> = files(result)
            .into_iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(got, vec!["lib.rs", "a.rs"]);
    }

    #[test]
    fn resolves_mod_dir_form() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("lib.rs");
        std::fs::write(&root, "pub mod a;\n").unwrap();
        std::fs::create_dir_all(dir.path().join("a")).unwrap();
        std::fs::write(dir.path().join("a").join("mod.rs"), "pub fn g() {}\n").unwrap();
        let result = module_tree(&target(&root)).unwrap();
        assert_eq!(files(result).len(), 2);
    }
    #[test]
    fn cfg_attr_path_resolves() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("lib.rs");
        std::fs::write(
            &root,
            "#[cfg_attr(a, path = \"other.rs\")]\npub mod renamed;\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("other.rs"), "pub fn g() {}\n").unwrap();
        let result = module_tree(&target(&root)).unwrap();
        assert_eq!(files(result).len(), 2);
    }

    #[test]
    fn cfg_attr_path_takes_the_existing_candidate() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("lib.rs");
        std::fs::write(
            &root,
            "#[cfg_attr(a, path = \"missing1.rs\")]\n\
             #[cfg_attr(b, path = \"present.rs\")]\n\
             pub mod m;\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("present.rs"), "pub fn g() {}\n").unwrap();
        let result = module_tree(&target(&root)).unwrap();
        let names: Vec<String> = files(result)
            .into_iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["lib.rs", "present.rs"]);
    }

    #[test]
    fn file_module_companion_directory() {
        // `de.rs` with a companion `de/` keeps its child modules in `de/`,
        // as in serde_derive.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("lib.rs");
        std::fs::write(&root, "pub mod de;\n").unwrap();
        std::fs::write(dir.path().join("de.rs"), "pub mod child;\n").unwrap();
        std::fs::create_dir_all(dir.path().join("de")).unwrap();
        std::fs::write(dir.path().join("de").join("child.rs"), "pub fn g() {}\n").unwrap();
        let result = module_tree(&target(&root)).unwrap();
        let names: Vec<String> = files(result)
            .into_iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["lib.rs", "de.rs", "child.rs"]);
    }

    #[test]
    fn path_attribute_overrides_name() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("lib.rs");
        std::fs::write(&root, "#[path = \"other.rs\"]\npub mod renamed;\n").unwrap();
        std::fs::write(dir.path().join("other.rs"), "pub fn g() {}\n").unwrap();
        let result = module_tree(&target(&root)).unwrap();
        let got: Vec<String> = files(result)
            .into_iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(got, vec!["lib.rs", "other.rs"]);
    }

    #[test]
    fn raw_identifier_module_file() {
        // `mod r#extern;` resolves to `extern.rs`, not `r#extern.rs`.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("lib.rs");
        std::fs::write(&root, "pub mod r#extern;\n").unwrap();
        std::fs::write(dir.path().join("extern.rs"), "pub fn g() {}\n").unwrap();
        let result = module_tree(&target(&root)).unwrap();
        assert_eq!(files(result).len(), 2);
    }

    #[test]
    fn path_attribute_uses_the_defining_directory() {
        // `#[path]` inside a file module is relative to the directory
        // containing that file, not its companion directory.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("lib.rs");
        std::fs::write(&root, "pub mod a;\n").unwrap();
        std::fs::write(dir.path().join("a.rs"), "#[path = \"a/b.rs\"]\nmod b;\n").unwrap();
        std::fs::create_dir_all(dir.path().join("a")).unwrap();
        std::fs::write(dir.path().join("a").join("b.rs"), "pub fn g() {}\n").unwrap();
        let result = module_tree(&target(&root)).unwrap();
        let names: Vec<String> = files(result)
            .into_iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["lib.rs", "a.rs", "b.rs"]);
    }

    #[test]
    fn missing_mod_is_skipped_without_error() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("lib.rs");
        std::fs::write(&root, "pub mod missing;\npub fn f() {}\n").unwrap();
        let result = module_tree(&target(&root)).unwrap();
        assert_eq!(files(result).len(), 1);
    }

    #[test]
    fn mod_decl_inside_inline_mod_uses_outer_dir() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("lib.rs");
        std::fs::write(&root, "pub mod a { pub mod c; }\n").unwrap();
        std::fs::write(dir.path().join("c.rs"), "pub fn g() {}\n").unwrap();
        let result = module_tree(&target(&root)).unwrap();
        assert_eq!(files(result).len(), 2);
    }

    #[test]
    fn module_cycles_terminate() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("lib.rs");
        std::fs::write(&root, "pub mod a;\n").unwrap();
        std::fs::write(
            dir.path().join("a.rs"),
            "#[path = \"lib.rs\"]\npub mod back;\n",
        )
        .unwrap();
        let result = module_tree(&target(&root)).unwrap();
        assert_eq!(files(result).len(), 2);
    }

    #[test]
    fn unparseable_file_is_skipped_without_error() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("lib.rs");
        std::fs::write(&root, "@@@ not rust @@ @\n").unwrap();
        let result = module_tree(&target(&root)).unwrap();
        assert!(result.is_empty());
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
            "[[bin]]\nname = \"tool\"\npath = \"src/bin/tool.rs\"\n",
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
            "[[bin]]\nname = \"tool\"\npath = \"src/bin/tool.rs\"\n",
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
