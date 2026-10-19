//! Workspace and module-tree discovery.

use alloc::collections::{BTreeMap, BTreeSet};
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
    /// Workspace package owning the target.
    pub(crate) package: String,
    /// Absolute path of the target's source root file.
    pub(crate) src: PathBuf,
    /// Whether its doctests are scanned.
    pub(crate) doc: bool,
    /// Whether it is a Rust library, whose public functions other crates may call.
    pub(crate) library: bool,
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
/// Fails when `cargo metadata` cannot be run for `root`, or when `package` names no workspace
/// member.
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
    let mut selected = false;
    for package in metadata
        .packages
        .iter()
        .filter(|p| package.is_none_or(|want| p.name == want))
    {
        selected = true;
        for target in &package.targets {
            targets.push(Target {
                name: target.name.clone(),
                package: package.name.to_string(),
                src: target.src_path.clone().into(),
                doc: scan_target(&target.kind, all_targets),
                library: target.kind.iter().any(|kind| {
                    matches!(
                        kind,
                        cargo_metadata::TargetKind::Lib
                            | cargo_metadata::TargetKind::RLib
                            | cargo_metadata::TargetKind::DyLib
                            | cargo_metadata::TargetKind::ProcMacro
                    )
                }),
            });
        }
    }
    if let Some(name) = package
        && !selected
    {
        return Err(crate::Error::UnknownPackage(name.to_string()));
    }
    Ok(Workspace {
        root: metadata.workspace_root.clone().into(),
        targets,
    })
}

/// Whether a cargo target's doctests are scanned, from its kind list.
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

pub(crate) struct ModuleTree {
    /// Every discovered file, those no configuration links included.
    pub(crate) files: Vec<crate::SourceFile>,
    /// Paths of files reached only through a module path no configuration links, such as a
    /// `cfg_attr(..., path = ...)` after an unconditional `path`.
    pub(crate) unlinked: BTreeSet<String>,
    pub(crate) failures: Vec<ModuleFailure>,
}

pub(crate) struct ModuleFailure {
    pub(crate) file: String,
    pub(crate) module: Vec<String>,
    pub(crate) message: String,
}

/// Parse available module files and retain discovery failures.
pub(crate) fn module_tree(target: &Target, boundary: Option<&std::path::Path>) -> ModuleTree {
    let mut tree = ModuleTree {
        files: Vec::new(),
        unlinked: BTreeSet::new(),
        failures: Vec::new(),
    };
    let root = Child {
        path: target.src.clone(),
        segments: Vec::new(),
        cfg: Vec::new(),
        mod_rs: true,
        rustdoc: target.doc,
        route: Route::Preferred,
    };
    let boundary = match boundary.map(std::fs::canonicalize).transpose() {
        Ok(boundary) => boundary,
        Err(error) => {
            record_failure(
                &root,
                format!("cannot establish source boundary ({error})"),
                &mut tree.failures,
            );
            return tree;
        }
    };
    let mut walk = Walk {
        boundary: boundary.as_deref(),
        tree: &mut tree,
        visited: BTreeSet::new(),
        off: alloc::collections::VecDeque::new(),
        alternates: alloc::collections::VecDeque::new(),
        unlinked: alloc::collections::VecDeque::new(),
    };
    walk.collect(root);
    // A file declared on both sides of a `cfg` must reach rustdoc first, a preferred module
    // path before any alternative branch, and a linked path before one never linked.
    while let Some(child) = walk
        .off
        .pop_front()
        .or_else(|| walk.alternates.pop_front())
        .or_else(|| walk.unlinked.pop_front())
    {
        walk.collect(child);
    }
    tree
}

/// One module-tree traversal.
struct Walk<'a> {
    boundary: Option<&'a std::path::Path>,
    tree: &'a mut ModuleTree,
    visited: BTreeSet<PathBuf>,
    /// Preferred module files rustdoc does not reach.
    off: alloc::collections::VecDeque<Child>,
    /// Files of module paths the build does not prefer.
    alternates: alloc::collections::VecDeque<Child>,
    /// Files of module paths no configuration links.
    unlinked: alloc::collections::VecDeque<Child>,
}

impl Walk<'_> {
    /// Collect `file` and the children rustdoc reaches, queueing the others.
    fn collect(&mut self, file: Child) {
        let failures = &mut self.tree.failures;
        let canonical = match std::fs::canonicalize(&file.path) {
            Ok(canonical) => canonical,
            Err(err) => {
                record_failure(
                    &file,
                    format!("cannot read {} ({err})", file.path.display()),
                    failures,
                );
                return;
            }
        };
        if self
            .boundary
            .is_some_and(|root| !canonical.starts_with(root))
        {
            record_failure(
                &file,
                format!(
                    "module source escapes the artifact boundary ({})",
                    file.path.display()
                ),
                failures,
            );
            return;
        }
        if !self.visited.insert(canonical) {
            return;
        }
        let text = match std::fs::read_to_string(&file.path) {
            Ok(text) => text,
            Err(err) => {
                record_failure(
                    &file,
                    format!("cannot read {} ({err})", file.path.display()),
                    failures,
                );
                return;
            }
        };
        let parsed = match syn::parse_file(&text) {
            Ok(parsed) => parsed,
            Err(err) => {
                record_failure(
                    &file,
                    format!("cannot parse {} ({err})", file.path.display()),
                    failures,
                );
                return;
            }
        };
        let dir = file.path.parent().unwrap_or(std::path::Path::new("."));
        let base = match file.path.file_stem() {
            Some(stem) if !file.mod_rs => dir.join(stem),
            _ => dir.to_path_buf(),
        };
        let mut children = Vec::new();
        mod_decls(
            &parsed.items,
            dir,
            &base,
            &Declaring {
                segments: &file.segments,
                cfg: &file.cfg,
                rustdoc: file.rustdoc,
                route: file.route,
            },
            &mut children,
            failures,
        );
        let path = file.path.to_string_lossy().into_owned();
        if file.route == Route::Unlinked {
            self.tree.unlinked.insert(path.clone());
        }
        self.tree.files.push(crate::SourceFile {
            path,
            segments: file.segments,
            cfg: file.cfg,
            parsed,
            text,
            rustdoc: file.rustdoc,
        });
        for child in children {
            match child.route {
                Route::Unlinked => self.unlinked.push_back(child),
                Route::Alternative => self.alternates.push_back(child),
                Route::Preferred if child.rustdoc => self.collect(child),
                Route::Preferred => self.off.push_back(child),
            }
        }
    }
}

fn record_failure(file: &Child, message: String, failures: &mut Vec<ModuleFailure>) {
    eprintln!("dejadoc: {message}");
    failures.push(ModuleFailure {
        file: file.path.to_string_lossy().into_owned(),
        module: file.segments.clone(),
        message,
    });
}

/// The module path, `cfg` attributes, rustdoc visibility and branch preference a list of items
/// inherits.
struct Declaring<'a> {
    segments: &'a [String],
    cfg: &'a [String],
    rustdoc: bool,
    route: Route,
}

/// The `mod name;` files at every nesting depth, `dir` resolving `#[path]` and `base` the
/// implicit files.
fn mod_decls(
    items: &[syn::Item],
    dir: &std::path::Path,
    base: &std::path::Path,
    declaring: &Declaring<'_>,
    out: &mut Vec<Child>,
    failures: &mut Vec<ModuleFailure>,
) {
    use syn::{Item, ext::IdentExt};

    for item in items {
        let Item::Mod(moditem) = item else {
            continue;
        };
        let rustdoc = declaring.rustdoc && crate::cfg::allows(&moditem.attrs);
        // A raw identifier names its file without the `r#`, the module
        // path keeps it as rustdoc does.
        let name = moditem.ident.unraw().to_string();
        let mut segments = declaring.segments.to_vec();
        let mut cfg = declaring.cfg.to_vec();
        cfg.extend(crate::cfg::cfgs(&moditem.attrs));
        let claims = crate::cfg::path_claims(&moditem.attrs, &|pred| {
            quote::ToTokens::to_token_stream(pred).to_string()
        });
        if let Some((_, children)) = &moditem.content {
            // A `#[path]` on an inline module names its directory, against the declaring file's.
            let preferred = declared_path(&moditem.attrs)
                .map_or_else(|| base.join(&name), |path| lexical_join(dir, &path));
            segments.push(moditem.ident.to_string());
            let implicit = [(base.join(&name), true)];
            for (inner, (_, conditions)) in candidates(&claims, dir, &implicit) {
                let mut cfg = cfg.clone();
                cfg.extend(link_entry(&conditions));
                mod_decls(
                    children,
                    &inner,
                    &inner,
                    &Declaring {
                        segments: &segments,
                        cfg: &cfg,
                        rustdoc: rustdoc && inner == preferred,
                        route: declaring
                            .route
                            .max(Route::of(inner == preferred, linked(&conditions))),
                    },
                    out,
                    failures,
                );
            }
            continue;
        }
        let preferred = resolve_mod_path(dir, base, &name, &moditem.attrs);
        segments.push(moditem.ident.to_string());
        let implicit = [
            (base.join(format!("{name}.rs")), false),
            (base.join(&name).join("mod.rs"), true),
        ];
        for (path, (mod_rs, conditions)) in candidates(&claims, dir, &implicit) {
            let required = rustdoc && path == preferred.0;
            let route = declaring
                .route
                .max(Route::of(path == preferred.0, linked(&conditions)));
            let mut child = Child {
                path,
                segments: segments.clone(),
                cfg: cfg.clone(),
                mod_rs,
                rustdoc: required,
                route,
            };
            child.cfg.extend(link_entry(&conditions));
            if child.path.exists() {
                out.push(child);
            } else if required {
                let message = format!("missing module file {}", child.path.display());
                record_failure(&child, message, failures);
            }
        }
    }
}

/// A module file declared by its parent.
struct Child {
    path: PathBuf,
    segments: Vec<String>,
    /// The `cfg` attributes of the declarations from the target root to this file.
    cfg: Vec<String>,
    /// A crate root, `mod.rs` or `#[path]` file, whose children sit beside it.
    mod_rs: bool,
    rustdoc: bool,
    route: Route,
}

/// How the build reaches a module file, ordered from the most to the least preferred.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Route {
    /// The path the doctest model links.
    Preferred,
    /// A path other configurations link.
    Alternative,
    /// A path no configuration links.
    Unlinked,
}

impl Route {
    /// The route of one candidate, by whether it is the preferred path and whether any
    /// configuration links it.
    fn of(preferred: bool, linked: bool) -> Self {
        if !linked {
            Self::Unlinked
        } else if preferred {
            Self::Preferred
        } else {
            Self::Alternative
        }
    }
}

/// The first module path whose expanded attribute applies.
fn declared_path(attrs: &[syn::Attribute]) -> Option<String> {
    use syn::Meta;

    let path = |meta: &Meta| match meta {
        Meta::NameValue(nv) if nv.path.is_ident("path") => {
            crate::cfg::lit_str(&nv.value).map(syn::LitStr::value)
        }
        _ => None,
    };
    attrs.iter().find_map(|attr| {
        path(&attr.meta).or_else(|| crate::cfg::cfg_attr_metas(attr)?.iter().find_map(path))
    })
}

/// Every file or directory a module declaration can link, each with whether it is a mod-rs
/// file and the conditions it links under, `implicit` holding the paths used without a `path`.
fn candidates(
    claims: &crate::cfg::PathClaims,
    dir: &std::path::Path,
    implicit: &[(PathBuf, bool)],
) -> BTreeMap<PathBuf, (bool, Vec<Vec<String>>)> {
    let mut out: BTreeMap<PathBuf, (bool, Vec<Vec<String>>)> = BTreeMap::new();
    let mut add = |path: PathBuf, mod_rs: bool, condition: &[String]| {
        out.entry(path)
            .or_insert_with(|| (mod_rs, Vec::new()))
            .1
            .push(condition.to_vec());
    };
    if let Some(path) = &claims.direct {
        add(lexical_join(dir, path), true, &claims.direct_pred);
    } else {
        for (path, mod_rs) in implicit {
            add(path.clone(), *mod_rs, &claims.fallback);
        }
    }
    for (condition, path) in &claims.branches {
        add(lexical_join(dir, path), true, condition);
    }
    out
}

/// Whether some configuration links a candidate, one of its conditions free of `false`.
fn linked(conditions: &[Vec<String>]) -> bool {
    conditions
        .iter()
        .any(|condition| !condition.iter().any(|part| part == "false"))
}

/// The printed `cfg` attribute under which a candidate links, `None` when it always links or
/// never does.
fn link_entry(conditions: &[Vec<String>]) -> Option<String> {
    let linkable: Vec<&Vec<String>> = conditions
        .iter()
        .filter(|condition| !condition.iter().any(|part| part == "false"))
        .collect();
    if linkable.is_empty() || linkable.iter().any(|condition| condition.is_empty()) {
        return None;
    }
    let rendered: Vec<String> = linkable
        .iter()
        .map(|condition| crate::cfg::render_conjunction(condition))
        .collect();
    let text = match rendered.as_slice() {
        [one] => one.clone(),
        _ => format!("any({})", rendered.join(", ")),
    };
    let pred: syn::Meta = syn::parse_str(&text).expect("printed cfg predicates parse back");
    Some(quote::quote!(#[cfg(#pred)]).to_string())
}

/// File for `mod name;` and whether it is a mod-rs file. A declared `#[path]` against `dir`,
/// otherwise `name.rs` or `name/mod.rs` under `base`.
fn resolve_mod_path(
    dir: &std::path::Path,
    base: &std::path::Path,
    name: &str,
    attrs: &[syn::Attribute],
) -> (PathBuf, bool) {
    if let Some(path) = declared_path(attrs) {
        return (lexical_join(dir, &path), true);
    }
    let plain = base.join(format!("{name}.rs"));
    if plain.exists() {
        return (plain, false);
    }
    (base.join(name).join("mod.rs"), true)
}

/// `dir` joined with a declared `path`, its `.` components dropped and each `..` taking off the
/// component before it, nothing past the root, so sites carry the path the repository shows.
fn lexical_join(dir: &std::path::Path, path: &str) -> PathBuf {
    use std::path::Component;

    let mut out = PathBuf::new();
    for component in dir.join(path).components() {
        match (component, out.components().next_back()) {
            (Component::CurDir, _)
            | (Component::ParentDir, Some(Component::RootDir | Component::Prefix(_))) => {}
            (Component::ParentDir, Some(Component::Normal(_))) => {
                out.pop();
            }
            (other, _) => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn target(src: &std::path::Path) -> Target {
        Target {
            name: "mycrate".into(),
            package: "mycrate".into(),
            src: src.to_path_buf(),
            doc: true,
            library: true,
        }
    }

    /// Write `files` into a tempdir, the first being the crate root, and
    /// walk its module tree, each file as its tempdir-relative path and
    /// its module path.
    fn walk(files: &[(&str, &str)]) -> Vec<(String, String)> {
        walk_filtered(files, false)
    }

    fn walk_filtered(files: &[(&str, &str)], rustdoc_only: bool) -> Vec<(String, String)> {
        let dir = tempfile::tempdir().unwrap();
        for (path, text) in files {
            let path = dir.path().join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        module_tree(&target(&dir.path().join(files[0].0)), None)
            .files
            .into_iter()
            .filter(|file| !rustdoc_only || file.rustdoc)
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
    fn records_the_cfg_chain_of_the_declarations() {
        let dir = tempfile::tempdir().unwrap();
        for (path, text) in [
            (
                "lib.rs",
                "#[cfg(unix)]\nmod outer {\n    #[cfg(feature = \"x\")]\n    pub mod inner;\n}\n#[cfg(windows)]\n#[path = \"w.rs\"]\nmod w;\nmod plain;\n",
            ),
            ("outer/inner.rs", "pub fn g() {}\n"),
            ("w.rs", "pub fn h() {}\n"),
            ("plain.rs", "pub fn p() {}\n"),
        ] {
            let path = dir.path().join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        let mut got: Vec<(String, Vec<String>)> =
            module_tree(&target(&dir.path().join("lib.rs")), None)
                .files
                .into_iter()
                .map(|file| {
                    let path = std::path::Path::new(&file.path)
                        .strip_prefix(dir.path())
                        .unwrap();
                    (path.to_string_lossy().into_owned(), file.cfg)
                })
                .collect();
        got.sort();
        assert_eq!(
            got,
            [
                ("lib.rs".to_string(), vec![]),
                (
                    "outer/inner.rs".to_string(),
                    vec![
                        "# [cfg (unix)]".to_string(),
                        "# [cfg (feature = \"x\")]".to_string()
                    ]
                ),
                ("plain.rs".to_string(), vec![]),
                ("w.rs".to_string(), vec!["# [cfg (windows)]".to_string()]),
            ]
        );
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
        let got: Vec<_> = walk_filtered(
            &[
                (
                    "lib.rs",
                    "#[cfg_attr(windows, path = \"windows.rs\")]\n\
                 #[cfg_attr(unix, path = \"unix.rs\")]\n\
                 pub mod platform;\n",
                ),
                ("windows.rs", "pub fn w() {}\n"),
                ("unix.rs", "pub fn u() {}\n"),
            ],
            true,
        )
        .into_iter()
        .map(|(path, _)| path)
        .collect();
        assert_eq!(got, ["lib.rs", "unix.rs"]);
    }

    #[test]
    fn cfg_attr_path_with_a_failing_predicate_falls_back_to_the_plain_file() {
        let got: Vec<_> = walk_filtered(
            &[
                (
                    "lib.rs",
                    "#[cfg_attr(windows, path = \"windows.rs\")]\npub mod platform;\n",
                ),
                ("windows.rs", "pub fn w() {}\n"),
                ("platform.rs", "pub fn p() {}\n"),
            ],
            true,
        )
        .into_iter()
        .map(|(path, _)| path)
        .collect();
        assert_eq!(got, ["lib.rs", "platform.rs"]);
    }

    #[test]
    fn rustdoc_preference_follows_nested_path_attribute_order() {
        for (attrs, preferred) in [
            (
                "#[cfg_attr(unix, cfg_attr(all(), path = \"alternate.rs\"))]\n#[path = \"default.rs\"]",
                "alternate.rs",
            ),
            (
                "#[path = \"default.rs\"]\n#[cfg_attr(unix, cfg_attr(all(), path = \"alternate.rs\"))]",
                "default.rs",
            ),
        ] {
            let source = format!("{attrs}\npub mod choice;\n");
            let files = [
                ("lib.rs", source.as_str()),
                ("default.rs", "pub fn default_helper() {}"),
                ("alternate.rs", "pub fn alternate_helper() {}"),
            ];
            let paths: Vec<_> = walk_filtered(&files, true)
                .into_iter()
                .map(|(path, _)| path)
                .collect();
            assert_eq!(paths, ["lib.rs", preferred]);
        }
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
        // File modules carry their module path, a raw-ident name spelled
        // with its `r#` as rustdoc names the test, and companion nesting.
        let modules: Vec<String> = walk(&[
            ("lib.rs", "pub mod r#extern;\npub mod a;\n"),
            ("extern.rs", "pub fn g() {}\n"),
            ("a.rs", "pub mod b;\n"),
            ("a/b.rs", "pub fn h() {}\n"),
        ])
        .into_iter()
        .map(|(_, module)| module)
        .collect();
        assert_eq!(modules, ["", "r#extern", "a", "a::b"]);
    }

    #[test]
    fn missing_mod_is_skipped_without_error() {
        let got = paths(&[("lib.rs", "pub mod missing;\npub fn f() {}\n")]);
        assert_eq!(got, ["lib.rs"]);
    }

    #[test]
    fn cfg_off_mod_files_are_collected_outside_rustdoc() {
        let dir = tempfile::tempdir().unwrap();
        for (path, text) in [
            (
                "lib.rs",
                "#[cfg(test)]\nmod tests;\n#[cfg(test)]\nmod inner { mod deep; }\n#[cfg(not(test))]\npub mod kept;\n#[cfg(windows)]\nmod absent;\n",
            ),
            ("tests.rs", "pub fn g() {}\n"),
            ("inner/deep.rs", "pub fn g() {}\n"),
            ("kept.rs", "pub fn g() {}\n"),
        ] {
            let path = dir.path().join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        let got: Vec<(String, bool)> = module_tree(&target(&dir.path().join("lib.rs")), None)
            .files
            .into_iter()
            .map(|f| (f.path.rsplit('/').next().unwrap().to_string(), f.rustdoc))
            .collect();
        let want = [
            ("lib.rs", true),
            ("kept.rs", true),
            ("tests.rs", false),
            ("deep.rs", false),
        ];
        assert_eq!(got, want.map(|(p, r)| (p.to_string(), r)));
        let mut lib = target(&dir.path().join("lib.rs"));
        lib.doc = false;
        assert!(module_tree(&lib, None).files.iter().all(|f| !f.rustdoc));
    }

    #[test]
    fn a_file_declared_under_exclusive_cfgs_stays_with_rustdoc() {
        // diesel declares `exists` once per side of a feature flag, the off one first.
        let dir = tempfile::tempdir().unwrap();
        for (path, text) in [
            (
                "lib.rs",
                "#[cfg(not(feature = \"x\"))]\npub(crate) mod exists;\n#[cfg(feature = \"x\")]\npub mod exists;\n",
            ),
            ("exists.rs", "#[cfg(test)]\nmod inner;\npub fn g() {}\n"),
            ("exists/inner.rs", "pub fn h() {}\n"),
        ] {
            let path = dir.path().join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        let got: Vec<(String, bool)> = module_tree(&target(&dir.path().join("lib.rs")), None)
            .files
            .into_iter()
            .map(|f| (f.path.rsplit('/').next().unwrap().to_string(), f.rustdoc))
            .collect();
        let want = [("lib.rs", true), ("exists.rs", true), ("inner.rs", false)];
        assert_eq!(got, want.map(|(p, r)| (p.to_string(), r)));
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
    fn lexical_join_keeps_a_parent_it_cannot_cancel() {
        let join = |dir: &str, path: &str| lexical_join(std::path::Path::new(dir), path);
        assert_eq!(join("a/b", "./../c.rs"), PathBuf::from("a/c.rs"));
        assert_eq!(join("a", "../../c.rs"), PathBuf::from("../c.rs"));
        assert_eq!(join("/", "../c.rs"), PathBuf::from("/c.rs"));
    }

    #[test]
    fn a_path_on_an_inline_mod_is_its_directory() {
        // num-bigint: an inline module whose files sit in a directory of another name.
        let got = paths(&[
            ("lib.rs", "mod bigrand;\n"),
            (
                "bigrand.rs",
                "#[path = \"bigrand\"]\nmod impl_a { mod traits; }\n",
            ),
            ("bigrand/traits.rs", "pub fn g() {}\n"),
        ]);
        assert_eq!(got, ["lib.rs", "bigrand.rs", "bigrand/traits.rs"]);
        // yew: `#[path = "."]` keeps the children beside the declaring `mod.rs`.
        let got = paths(&[
            ("lib.rs", "mod dom;\n"),
            (
                "dom/mod.rs",
                "#[cfg_attr(all(), path = \".\")]\nmod feat { #[path = \"./fragment.rs\"] mod fragment; }\n",
            ),
            ("dom/fragment.rs", "pub fn g() {}\n"),
        ]);
        assert_eq!(got, ["lib.rs", "dom/mod.rs", "dom/fragment.rs"]);
        // A `..` in a `#[path]` resolves lexically, so the site path is the repository's.
        let got = paths(&[
            ("lib.rs", "mod sys;\n"),
            ("sys/mod.rs", "#[path = \"../shared.rs\"]\nmod shared;\n"),
            ("shared.rs", "pub fn g() {}\n"),
        ]);
        assert_eq!(got, ["lib.rs", "sys/mod.rs", "shared.rs"]);
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
    fn every_target_is_listed_and_lib_targets_scan_doctests_by_default() {
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
        let targets: Vec<(&str, bool)> = ws
            .targets
            .iter()
            .map(|t| (t.name.as_str(), t.doc))
            .collect();
        assert_eq!(targets, [("ws", true), ("tool", false)]);
        assert!(ws.targets[0].src.ends_with("src/lib.rs"));
        assert_eq!(ws.root, dir.path().to_path_buf());
    }

    #[test]
    fn all_targets_scans_bin_and_example_doctests_never_test_targets() {
        let dir = tempfile::tempdir().unwrap();
        cargo_package(
            dir.path(),
            "ws",
            "[[bin]]\nname = \"tool\"\npath = \"src/bin/tool.rs\"\n[workspace]\n",
        );
        for (path, text) in [
            ("src/bin/tool.rs", "fn main() {}\n"),
            ("examples/demo.rs", "fn main() {}\n"),
            ("tests/it.rs", "#[test]\nfn t() {}\n"),
        ] {
            let path = dir.path().join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        let ws = workspace(dir.path(), None, true).unwrap();
        let mut targets: Vec<(&str, bool)> = ws
            .targets
            .iter()
            .map(|t| (t.name.as_str(), t.doc))
            .collect();
        targets.sort_unstable();
        assert_eq!(
            targets,
            [("demo", true), ("it", false), ("tool", true), ("ws", true)]
        );
    }

    #[test]
    fn a_target_file_missing_from_the_package_is_skipped() {
        // Published crates often leave out the benches and tests their manifest lists.
        let dir = tempfile::tempdir().unwrap();
        cargo_package(
            dir.path(),
            "ws",
            "[[bench]]\nname = \"gone\"\npath = \"benches/gone.rs\"\n[workspace]\n",
        );
        let ws = workspace(dir.path(), None, false).unwrap();
        let files: Vec<usize> = ws
            .targets
            .iter()
            .map(|t| module_tree(t, None).files.len())
            .collect();
        assert_eq!(files, [1, 0]);
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
        let err = workspace(dir.path(), Some("nope"), false).unwrap_err();
        assert_eq!(
            err.to_string(),
            "workspace: no workspace member named `nope`"
        );
    }

    #[test]
    fn proc_macro_targets_are_libraries() {
        let dir = tempfile::tempdir().unwrap();
        cargo_package(
            dir.path(),
            "ws",
            "[lib]\nproc-macro = true\n[[bin]]\nname = \"tool\"\npath = \"src/bin/tool.rs\"\n[workspace]\n",
        );
        let ws = workspace(dir.path(), None, false).unwrap();
        let targets: Vec<(&str, bool)> = ws
            .targets
            .iter()
            .map(|t| (t.name.as_str(), t.library))
            .collect();
        assert_eq!(targets, [("ws", true), ("tool", false)]);
    }

    #[test]
    fn conditional_module_paths_retain_available_default_and_alternate_sources() {
        let mut got = paths(&[
            (
                "lib.rs",
                "#[cfg_attr(feature = \"alternate\", path = \"alternate.rs\")] pub mod choice;",
            ),
            ("choice.rs", "pub fn default_helper() -> u8 { 11 }"),
            ("alternate.rs", "pub fn alternate_helper() -> u8 { 13 }"),
        ]);
        got.sort();
        assert_eq!(got, ["alternate.rs", "choice.rs", "lib.rs"]);
    }

    #[test]
    fn cfg_attr_paths_record_their_linking_condition() {
        let dir = tempfile::tempdir().unwrap();
        let lib = "#[cfg_attr(feature = \"a\", path = \"a.rs\")]\n\
            #[cfg_attr(feature = \"b\", path = \"b.rs\")]\n\
            mod pick;\n\
            #[cfg_attr(unix, path = \"u.rs\")]\n\
            #[path = \"direct.rs\"]\n\
            mod tail;\n\
            #[path = \"kept.rs\"]\n\
            #[cfg_attr(unix, path = \"dead.rs\")]\n\
            mod gone;\n\
            #[cfg_attr(unix, cfg_attr(feature = \"n\", path = \"nested.rs\"))]\n\
            mod deep;\n";
        std::fs::write(dir.path().join("lib.rs"), lib).unwrap();
        for file in [
            "a.rs",
            "b.rs",
            "pick.rs",
            "u.rs",
            "direct.rs",
            "kept.rs",
            "dead.rs",
            "nested.rs",
            "deep.rs",
        ] {
            std::fs::write(dir.path().join(file), "pub fn f() {}\n").unwrap();
        }
        let tree = module_tree(&target(&dir.path().join("lib.rs")), None);
        let relative = |path: &str| {
            std::path::Path::new(path)
                .strip_prefix(dir.path())
                .unwrap()
                .to_string_lossy()
                .into_owned()
        };
        let mut got: Vec<(String, Vec<String>)> = tree
            .files
            .iter()
            .filter(|file| !tree.unlinked.contains(&file.path))
            .map(|file| (relative(&file.path), file.cfg.clone()))
            .collect();
        got.sort();
        let cfg = |text: &str| vec![text.to_string()];
        assert_eq!(
            got,
            [
                ("a.rs".to_string(), cfg("# [cfg (feature = \"a\")]")),
                (
                    "b.rs".to_string(),
                    cfg("# [cfg (all (feature = \"b\" , not (feature = \"a\")))]")
                ),
                (
                    "deep.rs".to_string(),
                    cfg("# [cfg (not (all (unix , feature = \"n\")))]")
                ),
                ("direct.rs".to_string(), cfg("# [cfg (not (unix))]")),
                ("kept.rs".to_string(), vec![]),
                ("lib.rs".to_string(), vec![]),
                (
                    "nested.rs".to_string(),
                    cfg("# [cfg (all (unix , feature = \"n\"))]")
                ),
                (
                    "pick.rs".to_string(),
                    cfg("# [cfg (all (not (feature = \"a\") , not (feature = \"b\")))]")
                ),
                ("u.rs".to_string(), cfg("# [cfg (unix)]")),
            ]
        );
        let unlinked: Vec<String> = tree.unlinked.iter().map(|path| relative(path)).collect();
        assert_eq!(unlinked, ["dead.rs"]);
    }

    #[test]
    fn a_file_named_by_several_branches_links_under_any_of_them() {
        let dir = tempfile::tempdir().unwrap();
        let lib = "#[cfg_attr(feature = \"a\", path = \"x.rs\")]\n\
            #[cfg_attr(feature = \"b\", path = \"y.rs\")]\n\
            #[cfg_attr(doc, path = \"x.rs\")]\n\
            mod m;\n";
        for (file, text) in [("lib.rs", lib), ("x.rs", ""), ("y.rs", ""), ("m.rs", "")] {
            std::fs::write(dir.path().join(file), text).unwrap();
        }
        let tree = module_tree(&target(&dir.path().join("lib.rs")), None);
        let x = tree
            .files
            .iter()
            .find(|file| file.path.ends_with("x.rs"))
            .unwrap();
        assert_eq!(
            x.cfg,
            [
                "# [cfg (any (feature = \"a\" , all (doc , not (feature = \"a\") , not (feature = \"b\"))))]"
            ]
        );
    }

    #[test]
    fn conditional_inline_directories_retain_every_child_source() {
        let mut got = walk(&[
            (
                "lib.rs",
                "#[cfg_attr(feature = \"alternate\", path = \"alternate\")] pub mod outer { pub mod child; }",
            ),
            ("outer/child.rs", "pub fn default_helper() -> u8 { 11 }"),
            (
                "alternate/child.rs",
                "pub fn alternate_helper() -> u8 { 13 }",
            ),
        ]);
        got.sort();
        assert_eq!(
            got,
            [
                ("alternate/child.rs".into(), "outer::child".into()),
                ("lib.rs".into(), String::new()),
                ("outer/child.rs".into(), "outer::child".into()),
            ]
        );
    }

    #[test]
    fn missing_required_module_retains_a_discovery_failure() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("lib.rs"), "mod missing;").unwrap();
        let tree = module_tree(&target(&dir.path().join("lib.rs")), None);
        assert_eq!(tree.files.len(), 1);
        assert_eq!(tree.failures.len(), 1);
        assert_eq!(tree.failures[0].module, ["missing"]);
    }

    #[test]
    fn source_artifact_boundary_excludes_external_modules() {
        let dir = tempfile::tempdir().unwrap();
        let artifact = dir.path().join("artifact");
        std::fs::create_dir(&artifact).unwrap();
        std::fs::write(
            artifact.join("lib.rs"),
            "#[path = \"../shared.rs\"] mod shared;",
        )
        .unwrap();
        std::fs::write(dir.path().join("shared.rs"), "pub fn shared() -> u8 { 7 }").unwrap();
        let source = target(&artifact.join("lib.rs"));
        let bounded = module_tree(&source, Some(&artifact));
        assert_eq!(bounded.files.len(), 1);
        assert_eq!(bounded.failures.len(), 1);
        assert_eq!(bounded.failures[0].module, ["shared"]);
        assert_eq!(module_tree(&source, None).files.len(), 2);
    }

    #[test]
    fn records_a_boundary_that_cannot_be_canonicalized() {
        let missing = std::path::Path::new("/nonexistent/dejadoc-module-boundary");
        let tree = module_tree(&target(std::path::Path::new("lib.rs")), Some(missing));
        assert!(tree.files.is_empty());
        assert_eq!(tree.failures.len(), 1);
        assert!(
            tree.failures[0]
                .message
                .starts_with("cannot establish source boundary")
        );
    }

    #[test]
    fn records_a_root_file_that_is_not_utf8() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("lib.rs");
        std::fs::write(&root, [0xff, 0xfe, 0xfd]).unwrap();
        let tree = module_tree(&target(&root), None);
        assert!(tree.files.is_empty());
        assert_eq!(tree.failures.len(), 1);
        assert!(tree.failures[0].message.starts_with("cannot read "));
    }

    #[test]
    fn drops_a_cfg_attr_branch_that_does_not_parse() {
        let attr = |source: &str| {
            syn::parse_str::<syn::ItemFn>(&alloc::format!("{source}\nfn f() {{}}"))
                .unwrap()
                .attrs
                .pop()
                .unwrap()
        };
        let atom = |pred: &syn::Meta| quote::ToTokens::to_token_stream(pred).to_string();
        let broken = crate::cfg::path_claims(&[attr("#[cfg_attr(x y)]")], &atom);
        assert_eq!((broken.direct, broken.branches), (None, Vec::new()));
        let kept = attr("#[cfg_attr(feature = \"x\", path = \"other.rs\")]");
        let kept = crate::cfg::path_claims(&[kept], &atom);
        assert_eq!(
            kept.branches,
            [(vec!["feature = \"x\"".to_string()], "other.rs".to_string())]
        );
    }
}
