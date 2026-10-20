//! Context selection and dependency reuse through the CLI.

#![cfg(feature = "cli")]

use serde_json::Value;
use std::path::Path;
mod support;

#[path = "support/context_scan.rs"]
mod context_scan;

use context_scan::{group_items, scan};

fn put(root: &Path, path: &str, text: &str) {
    let file = root.join(path);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, text).unwrap();
}

fn fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    put(
        root,
        "Cargo.toml",
        "[workspace]\nresolver = \"3\"\nmembers = [\"app\", \"other\"]\nexclude = [\"deps/direct\", \"deps/transitive\", \"deps/development\", \"deps/optional\"]\n",
    );
    put(
        root,
        "app/Cargo.toml",
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[dependencies]\ndirect = { path = \"../deps/direct\" }\noptional = { path = \"../deps/optional\", optional = true }\n[dev-dependencies]\ndevelopment = { path = \"../deps/development\" }\n[features]\nwith-optional = [\"dep:optional\"]\n",
    );
    put(
        root,
        "app/src/lib.rs",
        "pub mod parser;\npub fn distant(value: u32) -> u32 { value + 7u32 }\npub fn development_use(value: u32) -> u32 { value + 11u32 }\npub fn optional_use(value: u32) -> u32 { value + 13u32 }\n",
    );
    put(
        root,
        "app/src/parser.rs",
        "pub fn selected(value: u32) -> u32 { let total = value + 1u32; total * 2u32 }\npub mod child { pub fn nested(input: u32) -> u32 { let result = input + 1u32; result * 2u32 } }\n",
    );
    put(
        root,
        "other/Cargo.toml",
        "[package]\nname = \"other\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    );
    put(
        root,
        "other/src/lib.rs",
        "pub fn available(input: u32) -> u32 { let result = input + 1u32; result * 2u32 }\n",
    );
    put(
        root,
        "deps/direct/Cargo.toml",
        "[package]\nname = \"direct\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[dependencies]\ntransitive = { path = \"../transitive\" }\n[workspace]\n",
    );
    put(
        root,
        "deps/direct/src/lib.rs",
        "pub fn helper(input: u32) -> u32 { let result = input + 1u32; result * 2u32 }\n",
    );
    for (name, increment) in [("transitive", 7), ("development", 11), ("optional", 13)] {
        put(
            root,
            &format!("deps/{name}/Cargo.toml"),
            &format!(
                "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[workspace]\n"
            ),
        );
        put(
            root,
            &format!("deps/{name}/src/lib.rs"),
            &format!("pub fn helper(input: u32) -> u32 {{ input + {increment}u32 }}\n"),
        );
    }
    dir
}

fn packages(report: &Value) -> Vec<String> {
    let mut names: Vec<String> = report["reuse"]["groups"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|group| group["candidates"].as_array().unwrap())
        .map(|candidate| candidate["package"].as_str().unwrap().to_owned())
        .collect();
    names.sort();
    names.dedup();
    names
}

#[test]
fn exact_module_selection_compares_against_another_crate() {
    let dir = fixture();
    let report = scan(
        dir.path(),
        &[
            "--context-select",
            "module-exact:app::parser",
            "--context-search",
            "crate:other",
        ],
    );
    assert_eq!(report["context_blocks"], 1);
    assert_eq!(
        group_items(&report),
        [vec![
            "app::parser::selected".to_owned(),
            "other::available".to_owned()
        ]]
    );
}

#[test]
fn module_subtree_and_physical_file_selection_include_inline_children() {
    let dir = fixture();
    let expected = [vec![
        "app::parser::child::nested".to_owned(),
        "app::parser::selected".to_owned(),
        "other::available".to_owned(),
    ]];
    for scope in ["module:app::parser", "file:app/src/parser.rs"] {
        let report = scan(
            dir.path(),
            &["--context-select", scope, "--context-search", "crate:other"],
        );
        assert_eq!(report["context_blocks"], 2);
        assert_eq!(group_items(&report), expected);
    }
}

#[test]
fn dependency_depth_and_kinds_control_reuse_candidates() {
    let dir = fixture();
    let cache = dir.path().join("index");
    let cache = cache.to_str().unwrap();
    let common = [
        "--context-select",
        "crate:app",
        "--context-check-latest",
        "false",
        "--context-cache-dir",
        cache,
    ];
    let mut args = common.to_vec();
    args.extend(["--context-dependency-depth", "1"]);
    assert_eq!(packages(&scan(dir.path(), &args)), ["direct"]);
    *args.last_mut().unwrap() = "2";
    assert_eq!(packages(&scan(dir.path(), &args)), ["direct", "transitive"]);
    args.extend(["--context-dev-dependencies"]);
    assert_eq!(
        packages(&scan(dir.path(), &args)),
        ["development", "direct", "transitive"]
    );
    args.extend(["--context-features", "with-optional"]);
    assert_eq!(
        packages(&scan(dir.path(), &args)),
        ["development", "direct", "optional", "transitive"]
    );
}

#[test]
fn source_changes_invalidate_a_shared_dependency_index() {
    let dir = fixture();
    let cache = dir.path().join("index");
    let cache = cache.to_str().unwrap();
    let args = [
        "--context-select",
        "crate:app",
        "--context-dependency-depth",
        "1",
        "--context-check-latest",
        "false",
        "--context-cache-dir",
        cache,
    ];
    assert_eq!(packages(&scan(dir.path(), &args)), ["direct"]);
    assert_eq!(packages(&scan(dir.path(), &args)), ["direct"]);
    put(
        dir.path(),
        "deps/direct/src/lib.rs",
        "pub fn helper(input: u32) -> u32 { input + 19u32 }\n",
    );
    assert_eq!(
        packages(&scan(dir.path(), &args)),
        [] as [std::string::String; 0]
    );
}

#[test]
fn dependency_configuration_fills_unset_cli_controls() {
    let dir = fixture();
    let cache = dir.path().join("index");
    put(
        dir.path(),
        ".dejadoc.toml",
        &format!(
            "context-select = \"crate:app\"\ncontext-search = \"crate:app\"\n[context-dependencies]\ndepth = 1\ninclude-dev = true\ncheck-latest = false\ncache-dir = '{}'\n",
            cache.display()
        ),
    );
    assert_eq!(packages(&scan(dir.path(), &[])), ["development", "direct"]);
    let report = scan(
        dir.path(),
        &[
            "--context-select",
            "module-exact:app::parser",
            "--context-search",
            "crate:other",
        ],
    );
    assert_eq!(packages(&report), ["direct"]);
    assert_eq!(
        group_items(&report),
        [vec![
            "app::parser::selected".to_owned(),
            "other::available".to_owned()
        ]]
    );
}

#[test]
fn context_detection_does_not_enable_dependency_search_implicitly() {
    let dir = fixture();
    let report = scan(dir.path(), &[]);
    assert!(report.get("reuse").is_none());
    assert_eq!(
        group_items(&report),
        [vec![
            "app::parser::child::nested".to_owned(),
            "app::parser::selected".to_owned(),
            "other::available".to_owned(),
        ]]
    );
}

#[test]
fn unreadable_selected_source_is_incomplete_even_under_no_fail() {
    let dir = fixture();
    put(dir.path(), "app/src/lib.rs", "pub fn unfinished(");
    let output = support::run_cli(
        dir.path(),
        &[
            "--context-select",
            "crate:app",
            "--context-search",
            "crate:app",
            "--context-dependency-depth",
            "1",
            "--context-check-latest",
            "false",
            "--no-fail",
            "--json",
        ],
    );
    assert_eq!(output.status.code(), Some(2));
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["reuse"]["coverage"]["complete"], false);
    assert!(
        report["reuse"]["coverage"]["failures"]
            .as_array()
            .unwrap()
            .iter()
            .any(|failure| failure.as_str().unwrap().contains("app/src/lib.rs"))
    );
}

#[test]
fn unrelated_crate_discovery_failure_does_not_poison_scoped_reuse() {
    let dir = fixture();
    put(dir.path(), "other/src/lib.rs", "pub fn unfinished(");
    let cache = dir.path().join("index");
    let report = scan(
        dir.path(),
        &[
            "--context-select",
            "crate:app",
            "--context-search",
            "crate:app",
            "--context-dependency-depth",
            "1",
            "--context-check-latest",
            "false",
            "--context-cache-dir",
            cache.to_str().unwrap(),
        ],
    );
    assert_eq!(report["reuse"]["coverage"]["complete"], true);
    assert_eq!(packages(&report), ["direct"]);
}

#[test]
fn explicit_context_scopes_are_independent_of_legacy_package_filtering() {
    let dir = fixture();
    let report = scan(
        dir.path(),
        &[
            "--package",
            "other",
            "--context-select",
            "module-exact:app::parser",
            "--context-search",
            "crate:other",
        ],
    );
    assert_eq!(
        group_items(&report),
        [vec![
            "app::parser::selected".to_owned(),
            "other::available".to_owned()
        ]]
    );
}

#[test]
fn a_physical_file_retains_its_second_module_membership() {
    let dir = fixture();
    put(
        dir.path(),
        "app/src/lib.rs",
        "mod first;\n#[path = \"first.rs\"]\npub mod second;\n",
    );
    put(
        dir.path(),
        "app/src/first.rs",
        "pub fn helper(input: u32) -> u32 { let result = input + 1u32; result * 2u32 }\n",
    );
    let report = scan(
        dir.path(),
        &[
            "--context-select",
            "module-exact:app::second",
            "--context-search",
            "crate:other",
        ],
    );
    assert_eq!(report["context_blocks"], 1);
    assert_eq!(
        group_items(&report),
        [vec![
            "app::second::helper".to_owned(),
            "other::available".to_owned()
        ]]
    );
}

#[test]
fn context_ranges_use_original_utf8_bytes_after_source_prefixes() {
    for prefix in ["\u{feff}", "#!/usr/bin/α\n", "\u{feff}#!/usr/bin/α\n"] {
        let dir = fixture();
        let function = "pub fn marker(λ: u8) -> u8 { λ + 1u8 }";
        let source = format!("{prefix}{function}\n");
        put(dir.path(), "app/src/lib.rs", &source);
        put(
            dir.path(),
            "other/src/lib.rs",
            "pub fn other(value: u8) -> u8 { value + 1u8 }\n",
        );
        let report = scan(
            dir.path(),
            &[
                "--context-select",
                "file:app/src/lib.rs",
                "--context-search",
                "crate:other",
            ],
        );
        let sites = report["context_groups"][0]["sites"].as_array().unwrap();
        let local = sites
            .iter()
            .find(|site| site["file"] == "app/src/lib.rs")
            .unwrap();
        let line = source.lines().last().unwrap();
        let number = source.lines().count();
        assert_eq!(local["line"], number, "{prefix:?}");
        assert_eq!(local["end"], number, "{prefix:?}");
        assert_eq!(local["column"], line.find('{').unwrap() + 1, "{prefix:?}");
        assert_eq!(
            local["end_column"],
            line.rfind('}').unwrap() + 2,
            "{prefix:?}"
        );
    }
}

#[test]
fn conditional_reexports_preserve_public_route_requirements() {
    let dir = fixture();
    put(
        dir.path(),
        "deps/direct/Cargo.toml",
        "[package]\nname = \"direct\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[features]\nexpose = []\n[workspace]\n",
    );
    put(
        dir.path(),
        "deps/direct/src/lib.rs",
        "mod hidden { pub fn helper(input: u32) -> u32 { let result = input + 1u32; result * 2u32 } }\n#[cfg(feature = \"expose\")]\npub use hidden::helper as exposed;\n",
    );
    let cache = dir.path().join("index");
    let report = scan(
        dir.path(),
        &[
            "--context-select",
            "module-exact:app::parser",
            "--context-search",
            "crate:app",
            "--context-dependency-depth",
            "1",
            "--context-check-latest",
            "false",
            "--context-cache-dir",
            cache.to_str().unwrap(),
        ],
    );
    let candidates = report["reuse"]["groups"][0]["candidates"]
        .as_array()
        .unwrap();
    let helper = candidates
        .iter()
        .find(|candidate| candidate["package"] == "direct")
        .unwrap();
    assert_eq!(helper["api_path"], "direct::exposed");
    assert!(
        helper["conditions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|condition| condition.as_str().unwrap().contains("expose"))
    );
}

#[test]
fn default_source_of_a_conditional_module_path_remains_selectable() {
    let dir = fixture();
    put(
        dir.path(),
        "app/src/lib.rs",
        "#[cfg_attr(feature = \"alternate\", path = \"alternate.rs\")]\npub mod choice;\n",
    );
    put(
        dir.path(),
        "app/src/choice.rs",
        "pub fn selected(value: u32) -> u32 { let total = value + 1u32; total * 2u32 }\n",
    );
    put(
        dir.path(),
        "app/src/alternate.rs",
        "pub fn different(value: u32) -> u32 { value + 17u32 }\n",
    );
    let report = scan(
        dir.path(),
        &[
            "--context-select",
            "file:app/src/choice.rs",
            "--context-search",
            "crate:other",
        ],
    );
    assert_eq!(report["context_blocks"], 1);
    assert_eq!(
        group_items(&report),
        [vec![
            "app::choice::selected".to_owned(),
            "other::available".to_owned()
        ]]
    );
}
