//! Context scope selection through the CLI.

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
        "[workspace]\nresolver = \"3\"\nmembers = [\"app\", \"other\"]\n",
    );
    put(
        root,
        "app/Cargo.toml",
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    );
    put(
        root,
        "app/src/lib.rs",
        "pub mod parser;\npub fn root_dup(value: u32) -> u32 { let result = value + 1u32; result * 2u32 }\npub fn solo(value: u32) -> u32 { value * 3u32 }\n",
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
        "pub fn available(input: u32) -> u32 { let result = input + 1u32; result * 2u32 }\npub fn twin_a(x: u32) -> u32 { x + 5u32 }\npub fn twin_b(x: u32) -> u32 { x + 5u32 }\n",
    );
    dir
}

fn dup_items() -> Vec<Vec<String>> {
    vec![vec![
        "app::parser::child::nested".to_owned(),
        "app::parser::selected".to_owned(),
        "app::root_dup".to_owned(),
        "other::available".to_owned(),
    ]]
}

#[test]
fn explicit_workspace_scope_matches_the_default_corpus() {
    let dir = fixture();
    let default = scan(dir.path(), &[]);
    let scoped = scan(
        dir.path(),
        &[
            "--context-select",
            "workspace",
            "--context-search",
            "workspace",
        ],
    );
    let mut expected = dup_items();
    expected.push(vec!["other::twin_a".to_owned(), "other::twin_b".to_owned()]);
    assert_eq!(group_items(&default), expected);
    assert_eq!(group_items(&scoped), expected);
}

#[test]
fn exact_module_selection_excludes_descendants() {
    let dir = fixture();
    let exact = scan(
        dir.path(),
        &[
            "--context-select",
            "module-exact:app::parser",
            "--context-search",
            "crate:other",
        ],
    );
    assert_eq!(
        group_items(&exact),
        vec![vec![
            "app::parser::selected".to_owned(),
            "other::available".to_owned()
        ]]
    );
    let subtree = scan(
        dir.path(),
        &[
            "--context-select",
            "module:app::parser",
            "--context-search",
            "crate:other",
        ],
    );
    assert_eq!(
        group_items(&subtree),
        vec![vec![
            "app::parser::child::nested".to_owned(),
            "app::parser::selected".to_owned(),
            "other::available".to_owned(),
        ]]
    );
}

#[test]
fn crate_scope_selects_by_target_name_or_root_path() {
    let dir = fixture();
    let by_name = scan(
        dir.path(),
        &[
            "--context-select",
            "crate:app",
            "--context-search",
            "crate:other",
        ],
    );
    assert_eq!(group_items(&by_name), dup_items());
    let by_root = scan(
        dir.path(),
        &[
            "--context-select",
            "crate:app/src/lib.rs",
            "--context-search",
            "crate:other",
        ],
    );
    assert_eq!(group_items(&by_root), dup_items());
    let absent = scan(
        dir.path(),
        &[
            "--context-select",
            "crate:app/src/nope.rs",
            "--context-search",
            "crate:other",
        ],
    );
    assert_eq!(absent["context_blocks"], 0);
    let empty: [Value; 0] = [];
    assert_eq!(absent["context_groups"].as_array().unwrap(), &empty);
}

#[test]
fn file_scope_selects_the_physical_file() {
    let dir = fixture();
    let report = scan(
        dir.path(),
        &[
            "--context-select",
            "file:app/src/parser.rs",
            "--context-search",
            "crate:other",
        ],
    );
    assert_eq!(
        group_items(&report),
        vec![vec![
            "app::parser::child::nested".to_owned(),
            "app::parser::selected".to_owned(),
            "other::available".to_owned(),
        ]]
    );
}

#[test]
fn every_group_needs_a_selected_local_anchor() {
    let dir = fixture();
    let local_only = scan(
        dir.path(),
        &[
            "--context-select",
            "crate:app",
            "--context-search",
            "crate:other",
        ],
    );
    let local_groups = local_only["context_groups"].as_array().unwrap();
    assert_eq!(local_groups.len(), 1);
    assert!(local_groups.iter().all(|group| {
        group["sites"]
            .as_array()
            .unwrap()
            .iter()
            .any(|site| site["item"].as_str().unwrap().starts_with("app::"))
    }));
    let corpus_only = scan(
        dir.path(),
        &[
            "--context-select",
            "crate:other",
            "--context-search",
            "crate:app",
        ],
    );
    assert_eq!(
        group_items(&corpus_only),
        vec![
            vec![
                "app::parser::child::nested".to_owned(),
                "app::parser::selected".to_owned(),
                "app::root_dup".to_owned(),
                "other::available".to_owned()
            ],
            vec!["other::twin_a".to_owned(), "other::twin_b".to_owned()],
        ]
    );
    let workspace = scan(
        dir.path(),
        &[
            "--context-select",
            "workspace",
            "--context-search",
            "workspace",
        ],
    );
    assert_eq!(
        group_items(&workspace),
        vec![
            vec![
                "app::parser::child::nested".to_owned(),
                "app::parser::selected".to_owned(),
                "app::root_dup".to_owned(),
                "other::available".to_owned()
            ],
            vec!["other::twin_a".to_owned(), "other::twin_b".to_owned()],
        ]
    );
}

#[test]
fn disjoint_comparison_scope_adds_sites_without_new_anchors() {
    let dir = fixture();
    let report = scan(
        dir.path(),
        &[
            "--context-select",
            "crate:other",
            "--context-search",
            "crate:app",
        ],
    );
    let groups = report["context_groups"].as_array().unwrap();
    assert_eq!(groups.len(), 2);
    let sites = groups
        .iter()
        .find(|group| {
            group["sites"]
                .as_array()
                .unwrap()
                .iter()
                .any(|site| site["item"] == "other::available")
        })
        .unwrap()["sites"]
        .as_array()
        .unwrap();
    let items: Vec<&str> = sites
        .iter()
        .map(|site| site["item"].as_str().unwrap())
        .collect();
    assert_eq!(items.len(), 4);
    assert!(items.contains(&"other::available"));
    assert!(items.contains(&"app::parser::selected"));
    assert!(items.contains(&"app::root_dup"));
    assert!(items.contains(&"app::parser::child::nested"));
    let anchor = sites
        .iter()
        .find(|site| site["item"] == "other::available")
        .unwrap();
    assert!(
        anchor["file"]
            .as_str()
            .unwrap()
            .ends_with("other/src/lib.rs")
    );
    let twin = groups
        .iter()
        .find(|group| {
            group["sites"]
                .as_array()
                .unwrap()
                .iter()
                .any(|site| site["item"] == "other::twin_a")
        })
        .unwrap();
    assert_eq!(twin["sites"].as_array().unwrap().len(), 2);
}

fn alias_fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    put(
        root,
        "Cargo.toml",
        "[workspace]\nresolver = \"3\"\nmembers = [\"app\", \"other\"]\n",
    );
    put(
        root,
        "app/Cargo.toml",
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    );
    put(
        root,
        "app/src/lib.rs",
        "mod first;\n#[path = \"first.rs\"]\npub mod second;\npub use second::helper;\n",
    );
    put(
        root,
        "app/src/first.rs",
        "pub fn helper(input: u32) -> u32 { let result = input + 1u32; result * 2u32 }\n",
    );
    put(
        root,
        "other/Cargo.toml",
        "[package]\nname = \"other\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    );
    put(
        root,
        "other/src/lib.rs",
        "pub fn helper(input: u32) -> u32 { let result = input + 1u32; result * 2u32 }\n",
    );
    dir
}

#[test]
fn an_alias_membership_reports_one_context_per_module_name() {
    let dir = alias_fixture();
    let by_first = scan(
        dir.path(),
        &[
            "--context-select",
            "module:app::first",
            "--context-search",
            "workspace",
        ],
    );
    assert_eq!(by_first["context_blocks"], 1);
    let by_second = scan(
        dir.path(),
        &[
            "--context-select",
            "module:app::second",
            "--context-search",
            "workspace",
        ],
    );
    assert_eq!(by_second["context_blocks"], 1);
    let whole = scan(
        dir.path(),
        &[
            "--context-select",
            "workspace",
            "--context-search",
            "workspace",
        ],
    );
    assert_eq!(whole["context_blocks"], 2);
    let grouped = scan(
        dir.path(),
        &[
            "--context-select",
            "module:app::second",
            "--context-search",
            "crate:other",
        ],
    );
    assert_eq!(
        group_items(&grouped),
        [vec![
            "app::second::helper".to_owned(),
            "other::helper".to_owned()
        ]]
    );
}
