use dejadoc::{Options, Report, run};
use std::path::Path;
use std::process::Command;

const FIXTURE: &str = "tests/fixtures/dupws";

fn scan() -> Report {
    run(Path::new(FIXTURE), &Options::default()).unwrap()
}

#[test]
fn shared_group_spans_crates_and_variants() {
    let report = scan();
    assert_eq!(report.total, 8);
    assert_eq!(report.unique, 4);
    assert_eq!(report.groups.len(), 2);
    let shared = report
        .groups
        .iter()
        .find(|g| !g.unparsed)
        .expect("shared group present");
    assert_eq!(shared.sites.len(), 3);
    let items: Vec<&str> = shared.sites.iter().map(|s| s.item.as_str()).collect();
    assert_eq!(
        items,
        vec!["alpha::alpha_fn", "alpha::alpha_variant", "beta::beta_fn"]
    );
    let files: Vec<&str> = shared
        .sites
        .iter()
        .map(|s| s.file.to_str().unwrap())
        .collect();
    assert_eq!(
        files,
        vec!["alpha/src/lib.rs", "alpha/src/lib.rs", "beta/src/lib.rs"]
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
    let opts = Options {
        config: Some(strict.clone()),
        ..Options::default()
    };
    let report = run(Path::new(FIXTURE), &opts).unwrap();
    assert!(report.groups.is_empty());

    // CLI threshold overrides the config file.
    let opts = Options {
        threshold: Some(2),
        config: Some(strict),
        ..Options::default()
    };
    let report = run(Path::new(FIXTURE), &opts).unwrap();
    assert_eq!(report.groups.len(), 2);
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
    assert_eq!(value["total"], 8);
    assert_eq!(value["groups"].as_array().unwrap().len(), 2);
}
