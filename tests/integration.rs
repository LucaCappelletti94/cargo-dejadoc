//! End-to-end tests of the scan through the real workspace fixtures.

#![cfg(feature = "std")]

use dejadoc::{ContextKind, Dejadoc, Remedy, Report};
use std::path::Path;
#[cfg(feature = "cli")]
use std::process::Command;

const FIXTURE: &str = "tests/fixtures/dupws";

fn scan() -> Report {
    dejadoc::Dejadoc::default().run(FIXTURE).unwrap()
}

const FN_FIXTURE: &str = "tests/fixtures/fnws";

/// A function group's items, self types and remedy.
type FnGroup<'a> = (Vec<&'a str>, Vec<Option<&'a str>>, Option<Remedy>);

#[test]
fn functions_group_per_module_across_targets_and_cfgs() {
    let report = Dejadoc::default().functions().run(FN_FIXTURE).unwrap();
    assert_eq!(
        (report.total, report.functions, report.unique_functions),
        (0, 17, 8)
    );
    let mut groups: Vec<FnGroup<'_>> = report
        .groups
        .iter()
        .map(|g| {
            assert_eq!(g.kind, dejadoc::Kind::Function);
            let items = g.sites.iter().map(|s| s.item.as_str()).collect();
            let types = g.sites.iter().map(|s| s.self_type.as_deref()).collect();
            (items, types, g.remedy)
        })
        .collect();
    groups.sort();
    assert_eq!(
        groups,
        [
            (
                vec![
                    "fnws::describe::Gamma::short",
                    "fnws::describe::Gamma::long"
                ],
                vec![Some("Gamma"), Some("Gamma")],
                Some(Remedy::HelperOrMacro)
            ),
            (
                vec!["fnws::platform::width", "fnws::platform::height"],
                vec![None, None],
                Some(Remedy::Delete)
            ),
            (
                vec!["fnws::render::Alpha::render", "fnws::render::Beta::render"],
                vec![Some("Alpha"), Some("Beta")],
                Some(Remedy::GenericOrMacro)
            ),
            (
                vec!["fnws::scale::scaled", "fnws::scale::scaled_elsewhere"],
                vec![None, None],
                Some(Remedy::MergeCfg)
            ),
            (
                vec![
                    "fnws::tests::totals_small_values",
                    "fnws::tests::totals_large_values"
                ],
                vec![None, None],
                Some(Remedy::Delete)
            ),
            (
                vec!["fnws::total_over", "fnws::sum_over", "fnws::total_too"],
                vec![None, None, None],
                Some(Remedy::Delete)
            ),
            (
                vec!["it::renders_values", "it::renders_values_again"],
                vec![None, None],
                Some(Remedy::Delete)
            ),
        ]
    );
    // A `pub` function of the library counts as public API whatever its module, since a
    // re-export can expose it. The integration test target has none.
    let mut public: Vec<&str> = report
        .groups
        .iter()
        .flat_map(|g| &g.sites)
        .filter(|s| s.public)
        .map(|s| s.item.as_str())
        .collect();
    public.sort_unstable();
    assert_eq!(
        public,
        [
            "fnws::platform::height",
            "fnws::platform::width",
            "fnws::render::Alpha::render",
            "fnws::render::Beta::render",
            "fnws::scale::scaled",
            "fnws::scale::scaled_elsewhere",
            "fnws::total_over",
            "fnws::total_too"
        ]
    );
    // A site runs from its first doc comment or attribute to its closing brace.
    for site in report.groups.iter().flat_map(|g| &g.sites) {
        let text = std::fs::read_to_string(Path::new(FN_FIXTURE).join(&site.file)).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        let first = lines[site.line as usize - 1].trim_start();
        let last = lines[site.end.unwrap() as usize - 1].trim();
        assert!(
            first.starts_with("///") || first.starts_with("#[") || first.contains("fn "),
            "{site:?}"
        );
        assert_eq!(last, "}", "{site:?}");
        assert_eq!(
            site.code.lines().count(),
            (site.end.unwrap() - site.line + 1) as usize
        );
    }
    let off = Dejadoc::default().run(FN_FIXTURE).unwrap();
    assert_eq!((off.functions, off.groups.len()), (0, 0));
}

#[test]
fn shared_group_spans_crates_and_variants() {
    let report = scan();
    assert_eq!(report.total, 9);
    assert_eq!(report.unique, 4);
    assert_eq!(report.groups.len(), 2);
    let shared = report
        .groups
        .iter()
        .find(|g| g.sites.iter().any(|s| s.item == "alpha::alpha_fn"))
        .expect("shared group present");
    assert_eq!(shared.sites.len(), 4);
    let items: Vec<&str> = shared.sites.iter().map(|s| s.item.as_str()).collect();
    // The `helper` copies name their item in the code and come first.
    assert_eq!(
        items,
        vec![
            "alpha::util::helper",
            "beta::util::helper",
            "alpha::alpha_fn",
            "alpha::alpha_variant"
        ]
    );
    let files: Vec<&str> = shared.sites.iter().map(|s| s.file.as_str()).collect();
    assert_eq!(
        files,
        vec![
            "alpha/src/util.rs",
            "beta/src/util.rs",
            "alpha/src/lib.rs",
            "alpha/src/lib.rs"
        ]
    );
}

#[test]
fn unparsed_pair_groups_with_flag() {
    let report = scan();
    let unparsed = report
        .groups
        .iter()
        .find(|g| g.unparsed)
        .expect("unparsed group present");
    assert_eq!(unparsed.sites.len(), 2);
}

#[test]
fn allowed_sites_are_not_grouped() {
    for group in scan().groups {
        for site in group.sites {
            assert!(!site.allow);
            assert_ne!(site.item, "alpha::alpha_allowed");
        }
    }
}

#[test]
fn config_threshold_suppresses_groups() {
    let strict = Path::new(FIXTURE).join(".dejadoc-strict.toml");
    let report = Dejadoc::default()
        .config(&strict)
        .unwrap()
        .run(FIXTURE)
        .unwrap();
    assert_eq!(report.groups, Vec::new());

    // API threshold overrides the config file.
    let report = Dejadoc::default()
        .threshold(2)
        .config(&strict)
        .unwrap()
        .run(FIXTURE)
        .unwrap();
    assert_eq!(report.groups.len(), 2);

    // The API threshold still wins when set after the config file.
    let report = Dejadoc::default()
        .config(&strict)
        .unwrap()
        .threshold(2)
        .run(FIXTURE)
        .unwrap();
    assert_eq!(report.groups.len(), 2);
}

#[test]
fn all_targets_builder_scans_bin_targets() {
    // The fixture's bin target (alpha/src/bin/dupbin.rs) is scanned only
    // with all_targets: one more doctest site, which alpha-renaming
    // joins into the function group.
    let report = Dejadoc::default().all_targets().run(FIXTURE).unwrap();
    assert_eq!(report.total, 10);
    assert_eq!(report.unique, 4);
    assert_eq!(report.groups.len(), 2);
}

#[test]
fn a_package_filter_keeps_the_package_bin_targets() {
    // `dupbin` is a target of package `alpha` with its own name.
    let report = Dejadoc::default()
        .package("alpha")
        .all_targets()
        .run(FIXTURE)
        .unwrap();
    assert!(
        report
            .groups
            .iter()
            .any(|g| g.sites.iter().any(|s| s.file.ends_with("dupbin.rs")))
    );
}

#[test]
fn min_tokens_builder_filters_blocks() {
    // min-tokens 4 drops the 3-token unparsed group; only the
    // 4-token function group survives.
    let report = Dejadoc::default().min_tokens(4).run(FIXTURE).unwrap();
    assert_eq!(report.total, 9);
    assert_eq!(report.groups.len(), 1);
}

#[test]
fn exit_code_reflects_duplicates() {
    let report = scan();
    assert_eq!(
        dejadoc::exit_code(&report, false),
        std::process::ExitCode::from(1)
    );
    assert_eq!(
        dejadoc::exit_code(&report, true),
        std::process::ExitCode::SUCCESS
    );
}
#[test]
#[cfg(feature = "cli")]
fn binary_runs_end_to_end() {
    let bin = env!("CARGO_BIN_EXE_cargo-dejadoc");
    let out = Command::new(bin)
        .current_dir(FIXTURE)
        .arg("dejadoc")
        .arg("--json")
        .output()
        .expect("run dejadoc binary");
    assert_eq!(out.status.code(), Some(1));
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid json output");
    assert_eq!(value["total"], 9);
    assert_eq!(value["groups"].as_array().unwrap().len(), 2);
}

#[test]
#[cfg(feature = "cli")]
fn binary_prints_annotations_beside_the_report() {
    let bin = env!("CARGO_BIN_EXE_cargo-dejadoc");
    let out = Command::new(bin)
        .current_dir(FIXTURE)
        .arg("dejadoc")
        .arg("--github")
        .output()
        .expect("run dejadoc binary");
    assert_eq!(out.status.code(), Some(1));
    let text = String::from_utf8(out.stdout).expect("utf8 output");
    assert!(text.contains("9 doctests (4 unique)"), "{text}");
    let marks: Vec<&str> = text.lines().filter(|l| l.starts_with("::")).collect();
    assert_eq!(marks.len(), 4, "{text}");
    assert!(
        marks
            .iter()
            .all(|l| l.starts_with("::error file=") && l.contains("title=dejadoc::")),
        "{text}"
    );
}

#[test]
#[cfg(feature = "cli")]
fn package_named_dejadoc_is_not_filtered() {
    // Only the cargo subcommand token at position 1 is stripped; a flag
    // value that happens to be "dejadoc" must survive to clap.
    let bin = env!("CARGO_BIN_EXE_cargo-dejadoc");
    let out = Command::new(bin)
        .current_dir(FIXTURE)
        .arg("dejadoc")
        .arg("--json")
        .arg("--package")
        .arg("dejadoc")
        .output()
        .expect("run dejadoc binary");
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {:?}",
        String::from_utf8_lossy(&out.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid json output");
    assert_eq!(value["total"], 0);
}

#[test]
fn file_module_items_carry_module_prefix() {
    // Items in file modules carry their module path, as rustdoc names
    // them (`util::helper`, not `helper`).
    let report = scan();
    let helper = report
        .groups
        .iter()
        .find(|g| g.sites.iter().any(|s| s.item.contains("helper")))
        .expect("helper group present");
    let items: Vec<&str> = helper
        .sites
        .iter()
        .map(|s| s.item.as_str())
        .filter(|item| item.contains("helper"))
        .collect();
    assert_eq!(items, vec!["alpha::util::helper", "beta::util::helper"]);
}

#[test]
fn threshold_builder_narrows_groups() {
    // Dupws has one 4-site group and one 2-site group: raising the
    // threshold through the builder keeps only the 4-site group.
    let report = dejadoc::Dejadoc::default()
        .threshold(3)
        .run(FIXTURE)
        .unwrap();
    assert_eq!(report.total, 9);
    assert_eq!(report.groups.len(), 1);
}

const CONTEXT_FIXTURE: &str = "tests/fixtures/contextws";

fn context_scan() -> Report {
    Dejadoc::default()
        .context_blocks()
        .context_min_tokens(0)
        .run(CONTEXT_FIXTURE)
        .unwrap()
}

fn context_items(group: &dejadoc::ContextGroup) -> Vec<&str> {
    group.sites.iter().map(|s| s.item.as_str()).collect()
}

#[test]
fn context_blocks_are_off_by_default() {
    let report = Dejadoc::default().run(CONTEXT_FIXTURE).unwrap();
    assert_eq!(report.context_blocks, 0);
    assert_eq!(report.unique_context_blocks, 0);
    assert_eq!(report.context_groups.len(), 0);
    assert_eq!(
        dejadoc::exit_code(&report, false),
        std::process::ExitCode::SUCCESS
    );
}

#[test]
fn renamed_locals_and_captures_group_across_functions() {
    let report = context_scan();
    let group = report
        .context_groups
        .iter()
        .find(|g| g.sites.iter().any(|s| s.item.ends_with("::count_up")))
        .expect("count_up context group");
    assert_eq!(
        context_items(group),
        vec!["contextws::count_up", "contextws::count_down"]
    );
    assert!(
        group
            .sites
            .iter()
            .all(|s| s.kind == ContextKind::FunctionBody)
    );
    assert!(group.sites.iter().all(|s| s.file.ends_with("src/lib.rs")));
}

#[test]
fn closure_captures_merge_and_distinct_references_do_not() {
    let report = context_scan();
    let closures = report
        .context_groups
        .iter()
        .find(|g| {
            g.sites.iter().all(|s| s.kind == ContextKind::Closure)
                && g.sites.iter().any(|s| s.item.ends_with("::mixed"))
        })
        .expect("closure group with the mixed extra site");
    // The repeated-reference closure in mixed stays out of this group,
    // so mixed contributes exactly one site.
    assert_eq!(
        context_items(closures),
        vec!["contextws::shifted", "contextws::moved", "contextws::mixed"]
    );
    assert!(report.context_groups.iter().all(|g| {
        g.sites
            .iter()
            .filter(|s| s.item.ends_with("::mixed"))
            .count()
            <= 1
    }));
    let functions = report
        .context_groups
        .iter()
        .find(|g| {
            g.sites.iter().all(|s| s.kind == ContextKind::FunctionBody)
                && g.sites.iter().any(|s| s.item.ends_with("::shifted"))
        })
        .expect("shifted/moved function group");
    assert_eq!(
        context_items(functions),
        vec!["contextws::shifted", "contextws::moved"]
    );
}

#[test]
fn distinct_import_targets_do_not_group() {
    let report = context_scan();
    assert!(report.context_groups.iter().all(|g| {
        !(g.sites.iter().any(|s| s.item.ends_with("::via_first"))
            && g.sites.iter().any(|s| s.item.ends_with("::via_second")))
    }));
}

#[test]
fn contained_block_group_survives_with_an_extra_site() {
    let report = context_scan();
    let blocks = report
        .context_groups
        .iter()
        .find(|g| {
            g.sites.iter().all(|s| s.kind == ContextKind::Block)
                && g.sites.iter().any(|s| s.item.ends_with("::bare"))
        })
        .expect("small block group with its extra site");
    assert_eq!(
        context_items(blocks),
        vec![
            "contextws::framed_left",
            "contextws::framed_right",
            "contextws::bare"
        ]
    );
    let functions = report
        .context_groups
        .iter()
        .find(|g| {
            g.sites.iter().all(|s| s.kind == ContextKind::FunctionBody)
                && g.sites.iter().any(|s| s.item.ends_with("::framed_left"))
        })
        .expect("framed function group");
    assert_eq!(
        context_items(functions),
        vec!["contextws::framed_left", "contextws::framed_right"]
    );
}

#[test]
fn same_line_context_sites_keep_their_columns() {
    let report = context_scan();
    let group = report
        .context_groups
        .iter()
        .find(|g| g.sites.iter().any(|s| s.item.ends_with("::paired")))
        .expect("paired closure group");
    assert_eq!(group.sites.len(), 2);
    assert!(group.sites.iter().all(|s| s.kind == ContextKind::Closure));
    let (a, b) = (&group.sites[0], &group.sites[1]);
    assert_eq!(a.line, b.line);
    assert_ne!(a.column, b.column);
    assert!(a.end_column <= b.column);
}

#[test]
fn the_default_context_floor_drops_the_fixture_contexts() {
    let report = Dejadoc::default()
        .context_blocks()
        .run(CONTEXT_FIXTURE)
        .unwrap();
    assert!(report.context_blocks > 0);
    assert_eq!(report.unique_context_blocks, 0);
    assert_eq!(report.context_groups.len(), 0);
}

#[test]
fn exit_code_reflects_context_groups() {
    let report = context_scan();
    assert_eq!(
        dejadoc::exit_code(&report, false),
        std::process::ExitCode::from(1)
    );
    assert_eq!(
        dejadoc::exit_code(&report, true),
        std::process::ExitCode::SUCCESS
    );
}

#[test]
fn config_file_enables_context_blocks() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(".dejadoc.toml");
    std::fs::write(&path, "context-blocks = true\ncontext-min-tokens = 0\n").unwrap();
    let report = Dejadoc::default()
        .config(&path)
        .unwrap()
        .run(CONTEXT_FIXTURE)
        .unwrap();
    assert!(
        report
            .context_groups
            .iter()
            .any(|g| g.sites.iter().any(|s| s.item.ends_with("::count_up")))
    );
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(".dejadoc.toml");
    std::fs::write(&path, "context-blocks = true\n").unwrap();
    let report = Dejadoc::default()
        .config(&path)
        .unwrap()
        .run(CONTEXT_FIXTURE)
        .unwrap();
    assert!(report.context_blocks > 0);
    assert_eq!(report.context_groups.len(), 0);
}

#[test]
#[cfg(feature = "cli")]
fn context_flags_run_the_scan_end_to_end() {
    let bin = env!("CARGO_BIN_EXE_cargo-dejadoc");
    let out = Command::new(bin)
        .current_dir(CONTEXT_FIXTURE)
        .args([
            "dejadoc",
            "--context-blocks",
            "--context-min-tokens",
            "0",
            "--json",
        ])
        .output()
        .expect("run dejadoc binary");
    assert_eq!(
        out.status.code(),
        Some(1),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid json output");
    assert!(
        value["context_blocks"]
            .as_u64()
            .expect("context block count")
            > 0
    );
    let groups = value["context_groups"].as_array().expect("context groups");
    for group in groups {
        assert!(group["id"].is_string());
        assert!(group["hash"].is_string());
        assert!(group["tokens"].is_u64());
        for site in group["sites"].as_array().expect("context sites") {
            assert!(site["file"].is_string());
            assert!(site["line"].is_u64());
            assert!(site["column"].is_u64());
            assert!(site["end"].is_u64());
            assert!(site["end_column"].is_u64());
            assert!(site["item"].is_string());
            assert!(matches!(
                site["kind"].as_str(),
                Some("function-body" | "block" | "arm" | "closure")
            ));
        }
    }
    let renamed = groups
        .iter()
        .find(|group| {
            group["sites"]
                .as_array()
                .unwrap()
                .iter()
                .any(|site| site["item"] == "contextws::count_up")
        })
        .expect("renamed function group");
    let items: Vec<_> = renamed["sites"]
        .as_array()
        .unwrap()
        .iter()
        .map(|site| site["item"].as_str().unwrap())
        .collect();
    assert_eq!(items, ["contextws::count_up", "contextws::count_down"]);
}

#[test]
#[cfg(feature = "cli")]
fn context_default_floor_and_no_fail_from_the_command_line() {
    let bin = env!("CARGO_BIN_EXE_cargo-dejadoc");
    let out = Command::new(bin)
        .current_dir(CONTEXT_FIXTURE)
        .args(["dejadoc", "--context-blocks", "--json"])
        .output()
        .expect("run dejadoc binary");
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid json output");
    assert!(
        value["context_blocks"]
            .as_u64()
            .expect("context block count")
            > 0
    );
    assert_eq!(value["context_groups"].as_array().unwrap().len(), 0);
    let out = Command::new(bin)
        .current_dir(CONTEXT_FIXTURE)
        .args([
            "dejadoc",
            "--context-blocks",
            "--context-min-tokens",
            "0",
            "--no-fail",
        ])
        .output()
        .expect("run dejadoc binary");
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
#[cfg(feature = "cli")]
fn context_annotations_are_approximate_with_coordinates() {
    let bin = env!("CARGO_BIN_EXE_cargo-dejadoc");
    let out = Command::new(bin)
        .current_dir(CONTEXT_FIXTURE)
        .args([
            "dejadoc",
            "--context-blocks",
            "--context-min-tokens",
            "0",
            "--github",
        ])
        .output()
        .expect("run dejadoc binary");
    assert_eq!(
        out.status.code(),
        Some(1),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8(out.stdout).expect("utf8 output");
    let marks: Vec<std::collections::BTreeMap<_, _>> = text
        .lines()
        .filter_map(|line| line.strip_prefix("::error "))
        .map(|mark| {
            mark.split_once("::")
                .unwrap()
                .0
                .split(',')
                .filter_map(|field| field.split_once('='))
                .collect()
        })
        .collect();
    let paired: Vec<_> = marks
        .iter()
        .filter(|mark| mark.get("line") == Some(&"86"))
        .map(|mark| (mark["col"], mark["endLine"], mark["endColumn"]))
        .collect();
    assert_eq!(paired, [("19", "86", "32"), ("35", "86", "48")]);
    assert!(
        marks
            .iter()
            .all(|mark| mark.get("file") == Some(&"src/lib.rs"))
    );
}
