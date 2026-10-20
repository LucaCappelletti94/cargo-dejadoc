use serde_json::Value;
use std::path::Path;

/// Run the CLI with context blocks on and the given extra options, as JSON.
pub(super) fn scan(root: &Path, args: &[&str]) -> Value {
    let mut options = vec![
        "--context-blocks",
        "--context-min-tokens",
        "0",
        "--no-fail",
        "--json",
    ];
    options.extend_from_slice(args);
    let output = crate::support::run_cli(root, &options);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

/// The sorted context-group member lists of one JSON report.
pub(super) fn group_items(report: &Value) -> Vec<Vec<String>> {
    let mut groups: Vec<Vec<String>> = report["context_groups"]
        .as_array()
        .unwrap()
        .iter()
        .map(|group| {
            let mut items: Vec<String> = group["sites"]
                .as_array()
                .unwrap()
                .iter()
                .map(|site| site["item"].as_str().unwrap().to_owned())
                .collect();
            items.sort();
            items
        })
        .collect();
    groups.sort();
    groups
}
