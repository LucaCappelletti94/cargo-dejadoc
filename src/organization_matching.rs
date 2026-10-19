//! Behavioral regressions for organization-wide matching, over prepared corpora.

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use std::path::{Path, PathBuf};

use crate::discover::Target;
use crate::organization_source::{Corpus, PackageSource};
use crate::{
    ContextScope, Dejadoc, DocTest, Error, Group, Kind, OrganizationCoverage, OrganizationPackage,
    OrganizationRepository, Remedy, Report, SourceFile, TargetScan,
};

/// A function body counting 33 tokens, above the default floor.
const BODY: &str = "(values: &[u32], limit: u32) -> u32 { let mut total = 0; for value in values { if *value > limit { total += value * 2; } else { total -= 1; } } let doubled = total + limit; doubled }";

/// The doctest code both a doc comment and a doc include may carry.
const BLOCK: &str = "let x = 1;\nlet y = x + 1;\n";

/// The doctest code with doc comment prefixes.
const BLOCK_DOC: &str = "/// let x = 1;\n/// let y = x + 1;\n";

/// A doc comment carrying the block.
const DOC_LIB: &str = "/// Doc.\n///\n/// ```\n/// let x = 1;\n/// let y = x + 1;\n/// ```\n";

/// One prepared repository snapshot and the packages discovered inside it.
struct RepoSpec {
    /// The GitHub repository name, `owner/name`.
    name: &'static str,
    /// The pinned default-branch commit, absent for an empty repository.
    commit: Option<&'static str>,
    /// The snapshot files, repository-relative.
    files: Vec<(String, String)>,
    /// The packages, each with its targets as (target name, repository-relative root file).
    packages: Vec<(String, Vec<(String, String)>)>,
}

/// A repository whose one package's lib is the single file `src/lib.rs`.
fn lib_repo(name: &'static str, package: &str, lib: &str) -> RepoSpec {
    RepoSpec {
        name,
        commit: Some("0123456789abcdef0123456789abcdef01234567"),
        files: vec![(String::from("src/lib.rs"), lib.to_string())],
        packages: vec![(
            package.to_string(),
            vec![(package.to_string(), String::from("src/lib.rs"))],
        )],
    }
}

/// Write the files of `repo` under `dir` and return the repository root.
fn snapshot(dir: &Path, repo: &RepoSpec) -> PathBuf {
    let root = dir.join(repo.name.replace('/', std::path::MAIN_SEPARATOR_STR));
    for (rel, text) in &repo.files {
        let path = root.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, text).unwrap();
    }
    root
}

/// The prepared corpus of `repos`, every snapshot written under `dir`.
fn corpus(dir: &Path, organization: &str, repos: &[RepoSpec]) -> Corpus {
    let mut repositories = Vec::new();
    let mut package_records = Vec::new();
    let mut roots = Vec::new();
    let mut package_sources = Vec::new();
    for repo in repos {
        let root = snapshot(dir, repo);
        let repository = repositories.len();
        repositories.push(OrganizationRepository {
            name: repo.name.to_string(),
            commit: repo.commit.map(str::to_string),
        });
        for (package_name, targets) in &repo.packages {
            let package = package_records.len();
            package_records.push(OrganizationPackage {
                repository,
                name: package_name.clone(),
                manifest: String::from("Cargo.toml"),
            });
            let targets: Vec<Target> = targets
                .iter()
                .map(|(name, rel)| Target {
                    name: name.clone(),
                    package: package_name.clone(),
                    src: root.join(rel),
                    doc: true,
                    library: true,
                })
                .collect();
            package_sources.push(PackageSource { package, targets });
        }
        roots.push(Some(root));
    }
    Corpus {
        report: crate::OrganizationReport {
            name: organization.to_string(),
            repositories,
            packages: package_records,
            file_packages: BTreeMap::new(),
            coverage: OrganizationCoverage {
                complete: true,
                failures: Vec::new(),
            },
        },
        roots,
        packages: package_sources,
    }
}

/// A one-file in-memory target, the local seam.
fn local_target(name: &str, path: &str, src: &str) -> TargetScan {
    TargetScan {
        name: name.to_string(),
        files: vec![SourceFile {
            path: path.to_string(),
            segments: Vec::new(),
            cfg: Vec::new(),
            parsed: syn::parse_str(src).unwrap(),
            text: src.to_string(),
            rustdoc: true,
        }],
        library: true,
    }
}

fn fn_groups(report: &Report) -> Vec<&Group> {
    report
        .groups
        .iter()
        .filter(|group| group.kind == Kind::Function)
        .collect()
}

fn doctest_groups(report: &Report) -> Vec<&Group> {
    report
        .groups
        .iter()
        .filter(|group| group.kind == Kind::Doctest)
        .collect()
}

#[test]
fn organization_functions_group_across_crate_and_module_names() {
    let dir = tempfile::tempdir().unwrap();
    let alpha = format!("mod one {{ fn shared{BODY} }}");
    let beta = format!("mod two {{ fn shared{BODY} }}");
    let corpus = corpus(
        dir.path(),
        "org",
        &[
            lib_repo("org/alpha", "alpha", &alpha),
            lib_repo("org/beta", "beta", &beta),
        ],
    );
    let report = Dejadoc::default()
        .functions()
        .run_organization_corpus(corpus)
        .unwrap();
    let groups = fn_groups(&report);
    assert_eq!(groups.len(), 1, "the scope leaves the organization hash");
    assert_eq!(
        groups[0]
            .sites
            .iter()
            .map(|site| site.item.as_str())
            .collect::<Vec<_>>(),
        ["alpha::one::shared", "beta::two::shared"]
    );
    assert_eq!(
        groups[0]
            .sites
            .iter()
            .map(|site| site.file.as_str())
            .collect::<Vec<_>>(),
        ["org/alpha/src/lib.rs", "org/beta/src/lib.rs"]
    );
    assert_eq!(report.organization.as_ref().unwrap().name, "org");
}

#[test]
fn organization_doctests_group_across_repositories() {
    let dir = tempfile::tempdir().unwrap();
    let alpha = format!(
        "{DOC_LIB}pub fn one() {{}}\n/// ```\n/// fn only_alpha() {{ 1 + 2 }}\n/// ```\npub fn two() {{}}\n"
    );
    let beta = format!(
        "{DOC_LIB}pub fn one() {{}}\n/// ```\n/// fn only_beta() {{ 3 + 4 }}\n/// ```\npub fn two() {{}}\n"
    );
    let corpus = corpus(
        dir.path(),
        "org",
        &[
            lib_repo("org/alpha", "alpha", &alpha),
            lib_repo("org/beta", "beta", &beta),
        ],
    );
    let report = Dejadoc::default().run_organization_corpus(corpus).unwrap();
    let groups = doctest_groups(&report);
    assert_eq!(
        groups.len(),
        1,
        "the shared block groups, the unique ones stay apart"
    );
    assert_eq!(
        groups[0]
            .sites
            .iter()
            .map(|site| site.file.as_str())
            .collect::<Vec<_>>(),
        ["org/alpha/src/lib.rs", "org/beta/src/lib.rs"]
    );
    assert_eq!(report.total, 4);
}

#[test]
fn local_function_matching_stays_within_one_crate_root() {
    let src = format!("fn shared{BODY}");
    let targets = [
        local_target("alpha", "alpha/src/lib.rs", &src),
        local_target("beta", "beta/src/lib.rs", &src),
    ];
    let report = Dejadoc::default()
        .functions()
        .run_targets("", &targets, &|_, _| None);
    assert!(
        fn_groups(&report).is_empty(),
        "the local hash keeps the module scope"
    );
    assert!(report.organization.is_none());
    let block = DocTest {
        file: String::from("a.rs"),
        line: 1,
        end: None,
        item: String::from("m::f"),
        info: Vec::new(),
        code: String::from("let x = 1;"),
        allow: false,
        self_type: None,
        public: false,
    };
    assert!(crate::group(&[block], 2, 0).organization.is_none());
}

#[test]
fn cross_scope_copies_get_the_safe_remedy() {
    let plain = format!("fn shared{BODY}");
    let windows = format!("#[cfg(windows)]\nfn shared{BODY}");
    let unix = format!("#[cfg(unix)]\nfn shared{BODY}");
    for (label, one, two) in [
        ("plain copies", plain.clone(), plain.clone()),
        ("cfg-apart copies", windows.clone(), unix.clone()),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let corpus = corpus(
            dir.path(),
            "org",
            &[lib_repo("org/a", "a", &one), lib_repo("org/b", "b", &two)],
        );
        let report = Dejadoc::default()
            .functions()
            .run_organization_corpus(corpus)
            .unwrap();
        let groups = fn_groups(&report);
        assert_eq!(groups.len(), 1, "{label}");
        assert_eq!(
            groups[0].remedy,
            Some(Remedy::HelperOrMacro),
            "{label} can't be deleted or cfg-merged across scopes"
        );
    }
}

#[test]
fn same_crate_copies_across_modules_get_the_safe_remedy() {
    let lib = format!("mod one {{ fn shared{BODY} }}\nmod two {{ fn shared{BODY} }}");
    let dir = tempfile::tempdir().unwrap();
    let corpus = corpus(dir.path(), "org", &[lib_repo("org/a", "a", &lib)]);
    let report = Dejadoc::default()
        .functions()
        .run_organization_corpus(corpus)
        .unwrap();
    let groups = fn_groups(&report);
    assert_eq!(groups.len(), 1, "another module is another scope");
    assert_eq!(
        groups[0].remedy,
        Some(Remedy::HelperOrMacro),
        "the copies can't be deleted across modules"
    );
}

#[test]
fn same_scope_copies_keep_the_local_remedy() {
    let lib = format!("#[cfg(unix)]\nfn shared{BODY}\n#[cfg(windows)]\nfn shared{BODY}");
    let dir = tempfile::tempdir().unwrap();
    let corpus = corpus(dir.path(), "org", &[lib_repo("org/a", "a", &lib)]);
    let report = Dejadoc::default()
        .functions()
        .run_organization_corpus(corpus)
        .unwrap();
    let groups = fn_groups(&report);
    assert_eq!(groups.len(), 1);
    assert_eq!(
        groups[0].remedy,
        Some(Remedy::MergeCfg),
        "one scope keeps the local remedy table"
    );
}

#[test]
fn cross_scope_methods_get_the_remedy_their_self_types_ask() {
    let scan = |alpha: &str, beta: &str| {
        let dir = tempfile::tempdir().unwrap();
        let corpus = corpus(
            dir.path(),
            "org",
            &[
                lib_repo("org/a", "alpha", alpha),
                lib_repo("org/b", "beta", beta),
            ],
        );
        Dejadoc::default()
            .functions()
            .run_organization_corpus(corpus)
            .unwrap()
    };
    let alpha = format!("struct A;\nimpl A {{ fn run{BODY} }}");
    let beta = format!("struct B;\nimpl B {{ fn run{BODY} }}");
    let report = scan(&alpha, &beta);
    let groups = fn_groups(&report);
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].remedy, Some(Remedy::GenericOrMacro));
    assert_eq!(
        groups[0]
            .sites
            .iter()
            .map(|site| site.self_type.as_deref())
            .collect::<Vec<_>>(),
        [Some("A"), Some("B")]
    );
    let alpha = format!("struct T;\nimpl T {{ fn run{BODY} }}");
    let beta = format!("struct T;\nimpl T {{ fn run{BODY} }}");
    let report = scan(&alpha, &beta);
    let groups = fn_groups(&report);
    assert_eq!(groups.len(), 1);
    assert_eq!(
        groups[0].remedy,
        Some(Remedy::HelperOrMacro),
        "same-named owners in different scopes stay one group"
    );
}

#[test]
fn same_named_owners_in_different_scopes_are_distinct_owners() {
    let dir = tempfile::tempdir().unwrap();
    let alpha = "struct Foo<T, U>(T, U);\nimpl<T: Default, U: Default> Foo<T, U> { fn pick() -> T { T::default() } }";
    let beta = "struct Foo<U, T>(U, T);\nimpl<T: Default, U: Default> Foo<U, T> { fn pick() -> T { T::default() } }";
    let corpus = corpus(
        dir.path(),
        "org",
        &[
            lib_repo("org/a", "alpha", alpha),
            lib_repo("org/b", "beta", beta),
        ],
    );
    let report = Dejadoc::default()
        .functions()
        .fn_min_tokens(0)
        .run_organization_corpus(corpus)
        .unwrap();
    let groups = fn_groups(&report);
    assert_eq!(
        groups.len(),
        1,
        "a same-named owner in another scope is a different owner"
    );
    assert_eq!(
        groups[0]
            .sites
            .iter()
            .map(|site| site.item.as_str())
            .collect::<Vec<_>>(),
        ["alpha::Foo<T,U>::pick", "beta::Foo<U,T>::pick"]
    );
    assert_eq!(groups[0].remedy, Some(Remedy::GenericOrMacro));
}

#[test]
fn a_physical_file_shared_by_two_packages_keeps_both_provenances() {
    let dir = tempfile::tempdir().unwrap();
    let lib = format!("{DOC_LIB}pub fn shared{BODY}\n");
    let other = lib_repo("org/other", "other", &lib);
    let shared = RepoSpec {
        name: "org/shared",
        commit: Some("0123456789abcdef0123456789abcdef01234567"),
        files: vec![(String::from("a/src/lib.rs"), lib)],
        packages: vec![
            (
                String::from("a"),
                vec![(String::from("a"), String::from("a/src/lib.rs"))],
            ),
            (
                String::from("b"),
                vec![(String::from("b"), String::from("a/src/lib.rs"))],
            ),
        ],
    };
    let corpus = corpus(dir.path(), "org", &[shared, other]);
    let report = Dejadoc::default()
        .functions()
        .run_organization_corpus(corpus)
        .unwrap();
    let org = report.organization.as_ref().unwrap();
    assert_eq!(org.packages.len(), 3);
    assert_eq!(
        org.file_packages.get("org/shared/a/src/lib.rs"),
        Some(&vec![0, 1]),
        "the shared file keeps every owning package"
    );
    assert_eq!(
        org.file_packages.get("org/other/src/lib.rs"),
        Some(&vec![2])
    );
    let functions = fn_groups(&report);
    assert_eq!(functions.len(), 1);
    assert_eq!(functions[0].sites.len(), 2);
    let doctests = doctest_groups(&report);
    assert_eq!(doctests.len(), 1);
    assert_eq!(
        doctests[0]
            .sites
            .iter()
            .map(|site| site.file.as_str())
            .collect::<Vec<_>>(),
        ["org/other/src/lib.rs", "org/shared/a/src/lib.rs"]
    );
}

#[test]
fn a_module_file_shared_by_two_crate_roots_is_one_function_occurrence() {
    let dir = tempfile::tempdir().unwrap();
    let root = String::from("#[path = \"../../common/m.rs\"]\nmod m;\n");
    let shared = RepoSpec {
        name: "org/shared",
        commit: Some("0123456789abcdef0123456789abcdef01234567"),
        files: vec![
            (
                String::from("common/m.rs"),
                format!("pub fn shared{BODY}\n"),
            ),
            (String::from("a/src/lib.rs"), root.clone()),
            (String::from("b/src/lib.rs"), root),
        ],
        packages: vec![
            (
                String::from("a"),
                vec![(String::from("a"), String::from("a/src/lib.rs"))],
            ),
            (
                String::from("b"),
                vec![(String::from("b"), String::from("b/src/lib.rs"))],
            ),
        ],
    };
    let other = RepoSpec {
        name: "org/other",
        commit: Some("0123456789abcdef0123456789abcdef01234567"),
        files: vec![
            (
                String::from("common/m.rs"),
                format!("pub fn shared{BODY}\n"),
            ),
            (
                String::from("c/src/lib.rs"),
                String::from("#[path = \"../../common/m.rs\"]\nmod m;\n"),
            ),
        ],
        packages: vec![(
            String::from("c"),
            vec![(String::from("c"), String::from("c/src/lib.rs"))],
        )],
    };
    let report = Dejadoc::default()
        .functions()
        .run_organization_corpus(corpus(dir.path(), "org", &[shared, other]))
        .unwrap();
    let org = report.organization.as_ref().unwrap();
    assert!(org.coverage.complete, "{:?}", org.coverage.failures);
    assert_eq!(
        report.functions, 2,
        "one physical function per file and line"
    );
    let groups = fn_groups(&report);
    assert_eq!(groups.len(), 1);
    assert_eq!(
        groups[0]
            .sites
            .iter()
            .map(|site| site.file.as_str())
            .collect::<Vec<_>>(),
        ["org/other/common/m.rs", "org/shared/common/m.rs"]
    );
    assert_eq!(
        org.file_packages.get("org/shared/common/m.rs"),
        Some(&vec![0, 1])
    );
}

#[test]
fn the_package_selector_restricts_the_organization_scan() {
    let dir = tempfile::tempdir().unwrap();
    let lib = format!("{DOC_LIB}pub fn one() {{}}\n");
    let corpus = corpus(
        dir.path(),
        "org",
        &[
            lib_repo("org/a", "a", &lib),
            lib_repo("org/b", "b", &lib),
            lib_repo("org/c", "c", &lib),
        ],
    );
    let selected = Dejadoc::default()
        .package("b")
        .run_organization_corpus(corpus)
        .unwrap();
    assert_eq!(selected.total, 1, "only the selected package is scanned");
    assert!(
        doctest_groups(&selected).is_empty(),
        "one site sits under the threshold"
    );
    assert_eq!(
        selected.organization.as_ref().unwrap().packages.len(),
        3,
        "provenance is not pruned"
    );
}

#[test]
fn an_unknown_organization_package_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = corpus(
        dir.path(),
        "org",
        &[lib_repo("org/a", "a", "pub fn f() {}")],
    );
    let result = Dejadoc::default()
        .package("absent")
        .run_organization_corpus(corpus);
    assert!(
        matches!(&result, Err(Error::UnknownOrganizationPackage(name)) if name == "absent"),
        "{result:?}"
    );
}

#[test]
fn missing_packages_in_incomplete_discovery_keep_coverage_failures() {
    let dir = tempfile::tempdir().unwrap();
    let mut corpus = corpus(
        dir.path(),
        "org",
        &[lib_repo("org/a", "a", "pub fn f() {}")],
    );
    corpus.report.coverage.complete = false;
    corpus
        .report
        .coverage
        .failures
        .push(String::from("org/unavailable"));
    let report = Dejadoc::default()
        .package("absent")
        .run_organization_corpus(corpus)
        .unwrap();
    assert!(!report.organization.as_ref().unwrap().coverage.complete);
    assert_eq!(
        crate::exit_code(&report, true),
        std::process::ExitCode::from(2)
    );
}

#[test]
fn included_source_text_keeps_doc_attributes_literal_in_both_target_orders() {
    for reverse in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let mut repo = RepoSpec {
            name: "org/source",
            commit: Some("0123456789abcdef0123456789abcdef01234567"),
            files: vec![
                (
                    String::from("src/a.rs"),
                    String::from("#[doc = include_str!(\"b.rs\")]\npub fn a() {}\n"),
                ),
                (
                    String::from("src/b.rs"),
                    String::from(
                        "/*\n```\nlet value = 1;\n```\n*/\n#[doc = include_str!(\"c.md\")]\npub fn b() {}\n",
                    ),
                ),
                (
                    String::from("src/c.md"),
                    String::from("```\nlet value = 9;\n```\n"),
                ),
            ],
            packages: vec![
                (
                    String::from("b"),
                    vec![(String::from("b"), String::from("src/b.rs"))],
                ),
                (
                    String::from("a"),
                    vec![(String::from("a"), String::from("src/a.rs"))],
                ),
            ],
        };
        if reverse {
            repo.packages.reverse();
        }
        let twin = lib_repo(
            "org/twin",
            "twin",
            "/// ```\n/// let other = 9;\n/// ```\npub fn f() {}\n",
        );
        let report = Dejadoc::default()
            .run_organization_corpus(corpus(dir.path(), "org", &[repo, twin]))
            .unwrap();
        let organization = report.organization.as_ref().unwrap();
        assert!(
            organization.coverage.complete,
            "{:?}",
            organization.coverage.failures
        );
        let owner = organization
            .packages
            .iter()
            .position(|package| package.name == "b")
            .unwrap();
        assert_eq!(
            organization.file_packages.get("org/source/src/c.md"),
            Some(&vec![owner]),
            "reverse={reverse}"
        );
    }
}

#[test]
fn explicit_context_scopes_keep_candidates_outside_the_legacy_package() {
    let dir = tempfile::tempdir().unwrap();
    let alpha = format!("fn one{BODY}");
    let beta = format!("fn two{BODY}\nfn three{BODY}");
    let repos = [
        lib_repo("org/a", "alpha", &alpha),
        lib_repo("org/b", "beta", &beta),
    ];
    let scanner = || {
        Dejadoc::default()
            .context_blocks()
            .context_min_tokens(0)
            .context_select(ContextScope::Module(String::from("beta")))
    };
    let all = scanner()
        .run_organization_corpus(corpus(dir.path(), "org", &repos))
        .unwrap();
    assert!(all.context_groups.iter().any(|group| {
        group
            .sites
            .iter()
            .any(|site| site.file == "org/b/src/lib.rs")
    }));
    let selected = scanner()
        .package("alpha")
        .run_organization_corpus(corpus(dir.path(), "org", &repos))
        .unwrap();
    assert_eq!(selected.context_groups, all.context_groups);
}

#[test]
fn cross_repository_context_blocks_group() {
    let dir = tempfile::tempdir().unwrap();
    let alpha = format!("fn one{BODY}");
    let beta = format!("fn two{BODY}");
    let corpus = corpus(
        dir.path(),
        "org",
        &[
            lib_repo("org/a", "alpha", &alpha),
            lib_repo("org/b", "beta", &beta),
        ],
    );
    let report = Dejadoc::default()
        .context_blocks()
        .context_min_tokens(0)
        .run_organization_corpus(corpus)
        .unwrap();
    assert!(report.context_groups.iter().any(|group| {
        group
            .sites
            .iter()
            .any(|site| site.file == "org/a/src/lib.rs")
            && group
                .sites
                .iter()
                .any(|site| site.file == "org/b/src/lib.rs")
    }));
}

#[test]
fn context_selection_restricts_organization_anchors() {
    let dir = tempfile::tempdir().unwrap();
    let alpha = format!("fn one{BODY}\nfn two{BODY}\n");
    let beta = "fn other() -> u8 { 1 + 2 }\n";
    let repos = [
        lib_repo("org/a", "alpha", &alpha),
        lib_repo("org/b", "beta", beta),
    ];
    let all = Dejadoc::default()
        .context_blocks()
        .context_min_tokens(0)
        .run_organization_corpus(corpus(dir.path(), "org", &repos))
        .unwrap();
    assert!(all.context_groups.iter().any(|group| {
        group.sites.iter().any(|site| site.item == "alpha::one")
            && group.sites.iter().any(|site| site.item == "alpha::two")
    }));
    let selected = Dejadoc::default()
        .context_blocks()
        .context_min_tokens(0)
        .context_select(ContextScope::Module(String::from("beta")))
        .run_organization_corpus(corpus(dir.path(), "org", &repos))
        .unwrap();
    assert!(
        selected.context_groups.is_empty(),
        "the alpha copies anchor no group"
    );
}

#[test]
fn confined_doc_include_qualifies_its_path_and_keeps_provenance() {
    let dir = tempfile::tempdir().unwrap();
    let alpha = RepoSpec {
        name: "org/alpha",
        commit: Some("0123456789abcdef0123456789abcdef01234567"),
        files: vec![
            (
                String::from("src/lib.rs"),
                "#[doc = include_str!(\"../docs/guide.md\")]\npub fn shared() {}\n".to_string(),
            ),
            (
                String::from("docs/guide.md"),
                format!("intro\n\n```\n{BLOCK}```\n"),
            ),
        ],
        packages: vec![(
            String::from("alpha"),
            vec![(String::from("alpha"), String::from("src/lib.rs"))],
        )],
    };
    let beta = format!("/// Doc.\n///\n/// ```\n{BLOCK_DOC}/// ```\npub fn shared() {{}}\n");
    let corpus = corpus(
        dir.path(),
        "org",
        &[alpha, lib_repo("org/beta", "beta", &beta)],
    );
    let report = Dejadoc::default().run_organization_corpus(corpus).unwrap();
    let groups = doctest_groups(&report);
    assert_eq!(
        groups.len(),
        1,
        "the included block groups with the written copy"
    );
    assert_eq!(
        groups[0]
            .sites
            .iter()
            .map(|site| site.file.as_str())
            .collect::<Vec<_>>(),
        ["org/alpha/docs/guide.md", "org/beta/src/lib.rs"]
    );
    assert_eq!(
        report
            .organization
            .as_ref()
            .unwrap()
            .file_packages
            .get("org/alpha/docs/guide.md"),
        Some(&vec![0]),
        "the included file keeps the including package's provenance"
    );
}

#[cfg(unix)]
#[test]
fn doc_includes_preserve_literal_backslashes_in_repository_paths() {
    let dir = tempfile::tempdir().unwrap();
    let alpha = RepoSpec {
        name: "org/alpha",
        commit: Some("0123456789abcdef0123456789abcdef01234567"),
        files: vec![
            (
                String::from("nested\\dir/lib.rs"),
                String::from("#[doc = include_str!(\"guide.md\")]\npub fn f() {}\n"),
            ),
            (
                String::from("nested\\dir/guide.md"),
                format!("```\n{BLOCK}```\n"),
            ),
        ],
        packages: vec![(
            String::from("alpha"),
            vec![(String::from("alpha"), String::from("nested\\dir/lib.rs"))],
        )],
    };
    let beta = lib_repo("org/beta", "beta", &format!("{DOC_LIB}pub fn f() {{}}\n"));
    let report = Dejadoc::default()
        .run_organization_corpus(corpus(dir.path(), "org", &[alpha, beta]))
        .unwrap();
    let organization = report.organization.as_ref().unwrap();
    assert!(
        organization.coverage.complete,
        "{:?}",
        organization.coverage.failures
    );
    let groups = doctest_groups(&report);
    assert_eq!(groups.len(), 1);
    assert!(
        groups[0]
            .sites
            .iter()
            .any(|site| site.file == "org/alpha/nested\\dir/guide.md")
    );
    assert_eq!(
        organization
            .file_packages
            .get("org/alpha/nested\\dir/guide.md"),
        Some(&vec![0])
    );
}

#[test]
fn a_doc_include_escaping_the_snapshot_marks_coverage_incomplete() {
    let dir = tempfile::tempdir().unwrap();
    let missing = lib_repo(
        "org/missing",
        "missing",
        "#[doc = include_str!(\"docs/nope.md\")]\npub fn shared() {}\n",
    );
    let escape = lib_repo(
        "org/escape",
        "escape",
        "#[doc = include_str!(\"../../outside.md\")]\npub fn shared() {}\n",
    );
    std::fs::write(dir.path().join("outside.md"), "```\nlet x = 1;\n```\n").unwrap();
    let good = lib_repo(
        "org/good",
        "good",
        &format!("{DOC_LIB}pub fn shared() {{}}\n"),
    );
    let keep = lib_repo(
        "org/keep",
        "keep",
        &format!("{DOC_LIB}pub fn shared() {{}}\n"),
    );
    let corpus = corpus(dir.path(), "org", &[missing, escape, good, keep]);
    let report = Dejadoc::default().run_organization_corpus(corpus).unwrap();
    let org = report.organization.as_ref().unwrap();
    assert!(
        !org.coverage.complete,
        "an unconfined include leaves the corpus incomplete"
    );
    assert_eq!(
        org.coverage.failures.len(),
        2,
        "{:?}",
        org.coverage.failures
    );
    let failure = |repo: &str| {
        org.coverage
            .failures
            .iter()
            .find(|failure| failure.starts_with(repo))
            .unwrap_or_else(|| panic!("no {repo} failure in {:?}", org.coverage.failures))
    };
    assert!(failure("org/missing/src/lib.rs").contains("docs/nope.md"));
    assert!(failure("org/escape/src/lib.rs").contains("../../outside.md"));
    assert_eq!(
        doctest_groups(&report).len(),
        1,
        "the rest of the corpus still groups"
    );
    for group in &report.groups {
        for site in &group.sites {
            assert!(
                !site.file.starts_with("org/escape") && !site.file.starts_with("org/missing"),
                "{:?}",
                site.file
            );
        }
    }
}

#[test]
fn a_module_failure_marks_coverage_and_keeps_the_rest() {
    let dir = tempfile::tempdir().unwrap();
    let bad = lib_repo(
        "org/bad",
        "bad",
        &format!("pub mod missing;\npub fn shared{BODY}\n"),
    );
    let good = lib_repo("org/good", "good", &format!("pub fn shared{BODY}"));
    let corpus = corpus(dir.path(), "org", &[bad, good]);
    let report = Dejadoc::default()
        .functions()
        .run_organization_corpus(corpus)
        .unwrap();
    let org = report.organization.as_ref().unwrap();
    assert!(!org.coverage.complete);
    assert!(
        org.coverage
            .failures
            .iter()
            .any(|failure| failure.contains("org/bad/src/missing")),
        "the failure names the missing module: {:?}",
        org.coverage.failures
    );
    let groups = fn_groups(&report);
    assert_eq!(groups.len(), 1, "the available sources still group");
    assert_eq!(
        groups[0]
            .sites
            .iter()
            .map(|site| site.file.as_str())
            .collect::<Vec<_>>(),
        ["org/bad/src/lib.rs", "org/good/src/lib.rs"]
    );
}

#[test]
fn dependency_traversal_is_rejected_in_organization_mode() {
    let err = Dejadoc::default()
        .context_dependency_depth(1)
        .run_organization("org")
        .unwrap_err();
    assert!(matches!(err, Error::OrganizationDependencies));
    assert!(err.to_string().contains("organization"));
}

#[test]
fn an_organization_scan_without_dependency_search_proceeds() {
    let report = Dejadoc::default().run_organization("../evil").unwrap();
    let coverage = &report.organization.as_ref().unwrap().coverage;
    assert!(!coverage.complete);
    assert!(coverage.failures[0].contains("not a valid GitHub login"));
}

#[test]
fn an_empty_repository_is_a_valid_input() {
    let dir = tempfile::tempdir().unwrap();
    let empty = RepoSpec {
        name: "org/notes",
        commit: Some("0123456789abcdef0123456789abcdef01234567"),
        files: vec![(String::from("README.md"), String::from("# notes"))],
        packages: Vec::new(),
    };
    let corpus = corpus(dir.path(), "org", &[empty]);
    let report = Dejadoc::default().run_organization_corpus(corpus).unwrap();
    let org = report.organization.as_ref().unwrap();
    assert_eq!(org.repositories.len(), 1);
    assert_eq!(org.packages.len(), 0);
    assert!(org.coverage.complete);
    assert_eq!(report.groups.len(), 0);
    assert_eq!(
        crate::exit_code(&report, false),
        std::process::ExitCode::SUCCESS
    );
}

#[test]
fn organization_incomplete_coverage_exits_two_even_with_no_fail() {
    let clean = || {
        lib_repo(
            "org/a",
            "a",
            "/// ```\n/// fn one() {}\n/// ```\npub fn one() {}\n",
        )
    };
    let twin = lib_repo(
        "org/b",
        "b",
        "/// ```\n/// fn one() {}\n/// ```\npub fn one() {}\n",
    );
    let solo = lib_repo(
        "org/solo",
        "solo",
        "/// ```\n/// assert_eq!(1 + 1, 2);\n/// ```\npub fn solo() {}\n",
    );
    let dir = tempfile::tempdir().unwrap();
    let report = Dejadoc::default()
        .run_organization_corpus(corpus(dir.path(), "org", &[clean(), solo]))
        .unwrap();
    assert_eq!(report.groups.len(), 0);
    assert_eq!(
        crate::exit_code(&report, false),
        std::process::ExitCode::SUCCESS
    );
    let dir = tempfile::tempdir().unwrap();
    let report = Dejadoc::default()
        .run_organization_corpus(corpus(dir.path(), "org", &[clean(), twin]))
        .unwrap();
    assert_eq!(doctest_groups(&report).len(), 1);
    assert_eq!(
        crate::exit_code(&report, false),
        std::process::ExitCode::FAILURE
    );
    let dir = tempfile::tempdir().unwrap();
    let broken = corpus(
        dir.path(),
        "org",
        &[clean(), lib_repo("org/bad", "bad", "pub mod missing;\n")],
    );
    let report = Dejadoc::default().run_organization_corpus(broken).unwrap();
    assert!(!report.organization.as_ref().unwrap().coverage.complete);
    assert_eq!(
        crate::exit_code(&report, true),
        std::process::ExitCode::from(2)
    );
    assert_eq!(
        crate::exit_code(&report, false),
        std::process::ExitCode::from(2)
    );
}
