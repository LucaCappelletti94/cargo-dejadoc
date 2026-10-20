//! Dependency discovery driven by a scripted cargo, for the metadata failure paths.

use std::path::{Path, PathBuf};

use echoes::{DependencySearch, ReuseReport};

const MATCHING_BODY: &str = "let result = input + 1u32; result * 2u32";
const APP_ID_SUFFIX: &str = "#0.1.0";

/// The cargo shim script, selecting its fixture by environment.
const CARGO_SHIM: &str = r#"#!/bin/sh
nodeps=0
for a in "$@"; do
  [ "$a" = "--no-deps" ] && nodeps=1
done
if [ "$nodeps" = "1" ]; then
  case "${DJD_CARGO_MODE:-ok}" in
    fail-nodeps) echo boom >&2; exit 1 ;;
    no-packages) printf '%s' '{"version":1}'; exit 0 ;;
    no-manifest-path) cat "$DJD_NOMANIFEST_JSON"; exit 0 ;;
  esac
  cat "$DJD_MEMBERS_JSON"
  exit 0
fi
cat "$DJD_FULL_JSON"
exit 0
"#;

fn put(root: &Path, rel: &str, text: &str) {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&path, text).unwrap();
}

/// The workspace scanned by the public run pipeline.
fn fixture(root: &Path) {
    put(
        root,
        "Cargo.toml",
        "[workspace]\nresolver = \"3\"\nmembers = [\"app\"]\n",
    );
    put(
        root,
        "app/Cargo.toml",
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    );
    put(
        root,
        "app/src/lib.rs",
        &format!("pub fn helper(input: u32) -> u32 {{ {MATCHING_BODY} }}\n"),
    );
    for name in ["tdep", "plain", "side", "ghost"] {
        put(
            root,
            &format!("{name}/Cargo.toml"),
            &format!(
                "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[workspace]\n"
            ),
        );
        put(
            root,
            &format!("{name}/src/lib.rs"),
            &format!("pub fn {name}_fn(input: u32) -> u32 {{ {MATCHING_BODY} }}\n"),
        );
    }
}

fn app_id(root: &Path) -> String {
    format!("path+file://{}/app{APP_ID_SUFFIX}", root.display())
}

fn dep_json(name: &str, target: Option<&str>, path: &Path) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "source": null,
        "req": "*",
        "kind": null,
        "optional": true,
        "uses_default_features": true,
        "features": [],
        "target": target,
        "rename": null,
        "registry": null,
        "path": path.to_string_lossy()
    })
}

fn members_json(root: &Path, with_manifest_path: bool) -> String {
    let mut package = serde_json::json!({
        "name": "app",
        "version": "0.1.0",
        "id": app_id(root),
        "source": null,
        "targets": [
            {
                "name": "app",
                "kind": ["lib"],
                "crate_types": ["lib"],
                "src_path": format!("{}/app/src/lib.rs", root.display()),
                "edition": "2024"
            }
        ]
    });
    if with_manifest_path {
        package["manifest_path"] =
            serde_json::Value::String(format!("{}/app/Cargo.toml", root.display()));
    }
    serde_json::json!({
        "packages": [package],
        "workspace_members": [app_id(root)],
        "version": 1
    })
    .to_string()
}

fn side_package(root: &Path, ghost: &Path, valid: bool) -> serde_json::Value {
    let deps = if valid {
        serde_json::json!([dep_json("ghost", None, ghost)])
    } else {
        serde_json::json!("not-an-array")
    };
    serde_json::json!({
        "name": "side",
        "version": "0.1.0",
        "id": format!("path+file://{}/side{APP_ID_SUFFIX}", root.display()),
        "source": null,
        "dependencies": [],
        "targeted_dependencies": deps,
        "targets": [
            {
                "name": "side",
                "kind": ["lib"],
                "crate_types": ["lib"],
                "src_path": format!("{}/side/src/lib.rs", root.display())
            }
        ],
        "manifest_path": format!("{}/side/Cargo.toml", root.display()),
        "features": {}
    })
}

fn full_json(
    root: &Path,
    app_deps: &[serde_json::Value],
    packages: Vec<serde_json::Value>,
) -> String {
    let mut all = vec![serde_json::json!({
        "name": "app",
        "version": "0.1.0",
        "id": app_id(root),
        "source": null,
        "dependencies": app_deps,
        "targets": [
            {
                "name": "app",
                "kind": ["lib"],
                "crate_types": ["lib"],
                "src_path": format!("{}/app/src/lib.rs", root.display())
            }
        ],
        "manifest_path": format!("{}/app/Cargo.toml", root.display()),
        "features": {}
    })];
    all.extend(packages);
    serde_json::json!({
        "packages": all,
        "workspace_members": [app_id(root)],
        "resolve": { "nodes": [
            { "id": app_id(root), "deps": [], "dependencies": [], "features": [] }
        ] },
        "workspace_root": root.to_string_lossy(),
        "target_directory": format!("{}/target", root.display()),
        "metadata": null,
        "version": 1
    })
    .to_string()
}

fn options(cache: &Path, target: Option<String>, depth: usize) -> DependencySearch {
    DependencySearch {
        depth: Some(depth),
        all_optional: Some(true),
        check_latest: Some(false),
        cache_dir: Some(cache.to_string_lossy().into_owned()),
        target,
        ..DependencySearch::default()
    }
}

fn run_dependency(root: &Path, options: DependencySearch) -> ReuseReport {
    let report = echoes::Echoes::default()
        .context_blocks()
        .context_min_tokens(1)
        .context_dependencies(options)
        .run(root)
        .expect("the run succeeds");
    report.reuse.expect("the dependency report is present")
}

fn candidate_names(reuse: &ReuseReport) -> Vec<String> {
    let mut names: Vec<String> = reuse
        .groups
        .iter()
        .flat_map(|group| group.candidates.iter())
        .map(|candidate| candidate.package.clone())
        .collect();
    names.sort();
    names.dedup();
    names
}

fn failures(reuse: &ReuseReport) -> String {
    reuse.coverage.failures.join("\n")
}

/// The child side of a scenario, selected by the `DJD_SCENARIO` environment.
fn scenario_body() {
    let root = PathBuf::from(std::env::var("DJD_ROOT").expect("the fixture root"));
    let cache = root.join("index");
    let scenario = std::env::var("DJD_SCENARIO").expect("the scenario name");
    match scenario.as_str() {
        "missing_rustc" => {
            let reuse = run_dependency(&root, options(&cache, None, 1));
            assert!(
                failures(&reuse).contains("could not establish the effective target"),
                "{reuse:?}"
            );
            assert!(
                candidate_names(&reuse).contains(&"tdep".to_owned()),
                "{reuse:?}"
            );
            assert!(!reuse.coverage.complete, "{:?}", reuse.coverage);
        }
        "explicit_target" => {
            let reuse = run_dependency(
                &root,
                options(&cache, Some("x86_64-unknown-linux-gnu".to_owned()), 1),
            );
            assert!(
                failures(&reuse).contains(
                    "platform conditions could not be evaluated for \
                     x86_64-unknown-linux-gnu (rustc cfg unavailable)"
                ),
                "{reuse:?}"
            );
            assert_eq!(
                candidate_names(&reuse),
                vec!["plain".to_owned()],
                "{reuse:?}"
            );
            assert!(!reuse.coverage.complete, "{:?}", reuse.coverage);
        }
        "targeted_followed" => {
            let reuse = run_dependency(
                &root,
                options(&cache, Some("x86_64-unknown-linux-gnu".to_owned()), 2),
            );
            assert!(
                candidate_names(&reuse).contains(&"ghost".to_owned()),
                "{reuse:?}"
            );
            assert!(reuse.coverage.complete, "{:?}", reuse.coverage);
        }
        "targeted_skipped" => {
            let reuse = run_dependency(
                &root,
                options(&cache, Some("x86_64-unknown-linux-gnu".to_owned()), 2),
            );
            assert!(
                !candidate_names(&reuse).contains(&"ghost".to_owned()),
                "{reuse:?}"
            );
            assert!(reuse.coverage.complete, "{:?}", reuse.coverage);
        }
        "null_id" | "no_packages" => {
            let reuse = run_dependency(&root, options(&cache, None, 1));
            assert!(
                failures(&reuse).contains("parse cargo metadata"),
                "{reuse:?}"
            );
            assert!(reuse.groups.is_empty(), "{reuse:?}");
        }
        "fail_nodeps" => {
            let reuse = run_dependency(&root, options(&cache, None, 1));
            assert!(
                failures(&reuse).contains("cargo metadata --no-deps failed: boom"),
                "{reuse:?}"
            );
            assert!(reuse.groups.is_empty(), "{reuse:?}");
            assert!(!reuse.coverage.complete, "{:?}", reuse.coverage);
        }
        "nodeps_no_packages" => {
            let reuse = run_dependency(&root, options(&cache, None, 1));
            assert!(
                failures(&reuse).contains("no-deps metadata lists no packages"),
                "{reuse:?}"
            );
            assert!(!reuse.coverage.complete, "{:?}", reuse.coverage);
        }
        "no_manifest_path" => {
            let reuse = run_dependency(
                &root,
                options(&cache, Some("x86_64-unknown-linux-gnu".to_owned()), 1),
            );
            assert!(reuse.coverage.complete, "{:?}", reuse.coverage);
            assert!(reuse.coverage.failures.is_empty(), "{reuse:?}");
            assert!(reuse.groups.is_empty(), "{reuse:?}");
        }
        other => panic!("unknown scenario {other}"),
    }
    eprintln!("fake cargo ok {scenario}");
}

/// The parent side of a scenario. The fixture and the scripted JSON files live
/// under `root`; the re-executed child runs the analysis with the shim as its
/// only `cargo` and no `rustc` on its path.
fn scenario(test_name: &str, scenario: &str, root: &Path, env: &[(&str, String)]) {
    let dir = root.join("djd");
    std::fs::create_dir_all(&dir).unwrap();
    let bin = root.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let script = bin.join("cargo");
    std::fs::write(&script, CARGO_SHIM).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mut permissions = std::fs::metadata(&script).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&script, permissions).unwrap();
    }
    let mut child_env: Vec<(String, String)> = Vec::new();
    for (key, value) in env {
        if key.ends_with("_JSON") {
            let path = dir.join(format!("{key}.json"));
            std::fs::write(&path, value).unwrap();
            child_env.push((key.to_string(), path.to_string_lossy().into_owned()));
        } else {
            child_env.push((key.to_string(), value.clone()));
        }
    }
    child_env.push((
        "PATH".to_owned(),
        format!("{bin}:/usr/bin:/bin", bin = bin.display()),
    ));
    child_env.push(("DJD_SCENARIO".to_owned(), scenario.to_owned()));
    let output = reexec(test_name, root, &child_env).expect("the child spawns");
    let text = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        text.contains("fake cargo ok"),
        "stderr: {text}\nstdout: {stdout}"
    );
}

fn reexec(
    test_name: &str,
    root: &Path,
    env: &[(String, String)],
) -> std::io::Result<std::process::Output> {
    let exe = std::env::current_exe()?;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg(test_name);
    cmd.arg("--exact");
    cmd.arg("--nocapture");
    cmd.env("DJD_ROOT", root.to_string_lossy().into_owned());
    for (key, value) in env {
        cmd.env(key, value);
    }
    cmd.output()
}

/// The parent-side fixture root, built fresh per test.
fn fixture_root() -> PathBuf {
    let root = tempfile::tempdir().unwrap().keep();
    fixture(&root);
    root
}

#[test]
fn missing_rustc_skips_platform_filtering() {
    if std::env::var_os("DJD_SCENARIO").is_some() {
        scenario_body();
        return;
    }
    let root = fixture_root();
    scenario(
        "missing_rustc_skips_platform_filtering",
        "missing_rustc",
        &root,
        &[
            ("DJD_MEMBERS_JSON", members_json(&root, true)),
            (
                "DJD_FULL_JSON",
                full_json(
                    &root,
                    &[dep_json("tdep", Some("cfg(unix)"), &root.join("tdep"))],
                    Vec::new(),
                ),
            ),
        ],
    );
}

#[test]
fn explicit_target_without_rustc_records_platform_failures() {
    if std::env::var_os("DJD_SCENARIO").is_some() {
        scenario_body();
        return;
    }
    let root = fixture_root();
    scenario(
        "explicit_target_without_rustc_records_platform_failures",
        "explicit_target",
        &root,
        &[
            ("DJD_MEMBERS_JSON", members_json(&root, true)),
            (
                "DJD_FULL_JSON",
                full_json(
                    &root,
                    &[
                        dep_json("tdep", Some("cfg(unix)"), &root.join("tdep")),
                        dep_json("plain", None, &root.join("plain")),
                    ],
                    Vec::new(),
                ),
            ),
        ],
    );
}

#[test]
fn targeted_dependencies_are_followed() {
    if std::env::var_os("DJD_SCENARIO").is_some() {
        scenario_body();
        return;
    }
    let root = fixture_root();
    scenario(
        "targeted_dependencies_are_followed",
        "targeted_followed",
        &root,
        &[
            ("DJD_MEMBERS_JSON", members_json(&root, true)),
            (
                "DJD_FULL_JSON",
                full_json(
                    &root,
                    &[dep_json("side", None, &root.join("side"))],
                    vec![side_package(&root, &root.join("ghost"), true)],
                ),
            ),
        ],
    );
}

#[test]
fn unparseable_targeted_dependencies_are_skipped() {
    if std::env::var_os("DJD_SCENARIO").is_some() {
        scenario_body();
        return;
    }
    let root = fixture_root();
    scenario(
        "unparseable_targeted_dependencies_are_skipped",
        "targeted_skipped",
        &root,
        &[
            ("DJD_MEMBERS_JSON", members_json(&root, true)),
            (
                "DJD_FULL_JSON",
                full_json(
                    &root,
                    &[dep_json("side", None, &root.join("side"))],
                    vec![side_package(&root, &root.join("ghost"), false)],
                ),
            ),
        ],
    );
}

#[test]
fn metadata_with_a_null_package_id_fails_to_parse() {
    if std::env::var_os("DJD_SCENARIO").is_some() {
        scenario_body();
        return;
    }
    let root = fixture_root();
    let mut value: serde_json::Value =
        serde_json::from_str(&full_json(&root, &[], Vec::new())).unwrap();
    value["packages"][0]["id"] = serde_json::Value::Null;
    scenario(
        "metadata_with_a_null_package_id_fails_to_parse",
        "null_id",
        &root,
        &[
            ("DJD_MEMBERS_JSON", members_json(&root, true)),
            ("DJD_FULL_JSON", value.to_string()),
        ],
    );
}

#[test]
fn metadata_without_packages_fails_to_parse() {
    if std::env::var_os("DJD_SCENARIO").is_some() {
        scenario_body();
        return;
    }
    let root = fixture_root();
    scenario(
        "metadata_without_packages_fails_to_parse",
        "no_packages",
        &root,
        &[
            ("DJD_MEMBERS_JSON", members_json(&root, true)),
            (
                "DJD_FULL_JSON",
                format!(
                    "{{\"version\":1,\"workspace_root\":\"{}\"}}",
                    root.display()
                ),
            ),
        ],
    );
}

#[test]
fn nodeps_metadata_failure_is_reported() {
    if std::env::var_os("DJD_SCENARIO").is_some() {
        scenario_body();
        return;
    }
    let root = fixture_root();
    scenario(
        "nodeps_metadata_failure_is_reported",
        "fail_nodeps",
        &root,
        &[
            ("DJD_MEMBERS_JSON", members_json(&root, true)),
            ("DJD_FULL_JSON", full_json(&root, &[], Vec::new())),
            ("DJD_CARGO_MODE", "fail-nodeps".to_owned()),
        ],
    );
}

#[test]
fn nodeps_metadata_without_packages_is_reported() {
    if std::env::var_os("DJD_SCENARIO").is_some() {
        scenario_body();
        return;
    }
    let root = fixture_root();
    scenario(
        "nodeps_metadata_without_packages_is_reported",
        "nodeps_no_packages",
        &root,
        &[
            ("DJD_MEMBERS_JSON", members_json(&root, true)),
            ("DJD_FULL_JSON", full_json(&root, &[], Vec::new())),
            ("DJD_CARGO_MODE", "no-packages".to_owned()),
        ],
    );
}

#[test]
fn member_without_manifest_path_is_skipped() {
    if std::env::var_os("DJD_SCENARIO").is_some() {
        scenario_body();
        return;
    }
    let root = fixture_root();
    scenario(
        "member_without_manifest_path_is_skipped",
        "no_manifest_path",
        &root,
        &[
            ("DJD_NOMANIFEST_JSON", members_json(&root, false)),
            ("DJD_FULL_JSON", full_json(&root, &[], Vec::new())),
            ("DJD_CARGO_MODE", "no-manifest-path".to_owned()),
        ],
    );
}
