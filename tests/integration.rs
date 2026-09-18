//! End-to-end tests of the scan through the real workspace fixtures.

#![cfg(feature = "std")]

use dejadoc::{Dejadoc, Report};
use std::path::Path;
#[cfg(feature = "cli")]
use std::process::Command;

const FIXTURE: &str = "tests/fixtures/dupws";

fn scan() -> Report {
    dejadoc::Dejadoc::default().run(FIXTURE).unwrap()
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
    assert_eq!(
        items,
        vec![
            "alpha::alpha_fn",
            "alpha::alpha_variant",
            "alpha::util::helper",
            "beta::util::helper"
        ]
    );
    let files: Vec<&str> = shared.sites.iter().map(|s| s.file.as_str()).collect();
    assert_eq!(
        files,
        vec![
            "alpha/src/lib.rs",
            "alpha/src/lib.rs",
            "alpha/src/util.rs",
            "beta/src/util.rs"
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
    assert!(text.contains("9 doctests, 4 unique"), "{text}");
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
