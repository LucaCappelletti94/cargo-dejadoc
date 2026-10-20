//! Smoke harness for `action.yml`, ported from the Python harness.
//!
//! One test per scenario. Each runs the real `run` steps of `action.yml`
//! through a fake runner with fixture `cargo` and `gh` binaries on the
//! `PATH`, one fixture report, and a fake `github` context, and asserts on
//! the recorded calls, summaries, and status files.

#[path = "action/mod.rs"]
mod action;

use std::fs;
use std::path::Path;

use action::fix::{
    Cand, coverage, ctx_group, doc_site, group, issue_url, report, reuse, reuse_group, site,
};
use action::runner::{Runner, Use};
use action::world::{SRC_DUP, SRC_LIB, SRC_PARSER, World};
use serde_json::Value;

// actions/cache v4 and actions/cache save v4 resolve to this commit, verified
// against the actions/cache repository refs.
const CACHE_SHA: &str = "0057852bfaa89a56745cba8c7296529d2fc39830";
const CACHE_KEY_PREFIX: &str = "echoes-context-index-Linux-";

fn use_by_name<'a>(uses: &'a [Use], name: &str) -> Option<&'a Use> {
    uses.iter().find(|u| u.name == name)
}

fn joined_bodies(files: &std::collections::BTreeMap<String, String>) -> String {
    files.values().cloned().collect::<Vec<_>>().join("\n")
}

#[test]
fn s01_gate_clean() {
    let mut world = World::new("s01")
        .inputs(&[("install", "true")])
        .json(report(&[], &[], None, 6, 3))
        .human("0 doctests, 0 unique, 0 functions, 0 unique functions, 0 duplicated groups\n");
    let exit = world.run();
    let r = world.runner();
    assert_eq!(exit, 0, "clean gate run exits 0");
    assert!(
        r.summary().contains("### echoes"),
        "the run summary carries the echoes header"
    );
    assert!(
        r.summary().contains("duplicated groups"),
        "the run summary carries the scan line"
    );
    assert!(
        r.summary().contains("```"),
        "the gate summary fences the scan output"
    );
    let scan = r.scan_args(0);
    assert!(
        scan.iter().any(|a| a == "--github"),
        "annotations are requested in gate mode"
    );
    assert!(
        !scan.iter().any(|a| a == "--context-dependency-depth"),
        "no dependency flags at depth zero"
    );
    let install: Vec<Value> = r
        .cargo_calls()
        .into_iter()
        .filter(|c| c.get("cmd").and_then(Value::as_str) == Some("install"))
        .collect();
    assert_eq!(
        install.len(),
        1,
        "the install step used the fake cargo install"
    );
    assert_eq!(r.comment_posts().len(), 0, "no review comments are posted");
    assert_eq!(r.uses().len(), 0, "no cache step runs at depth zero");
}

#[test]
fn s02_gate_dups() {
    let mut world = World::new("s02")
        .inputs(&[("cwd", "pkg")])
        .json(report(
            &[group(
                "g1",
                "H1",
                &[
                    doc_site("src/lib.rs", 3, 5, "app::parse"),
                    doc_site("src/dup.rs", 3, 5, "app::parse"),
                ],
            )],
            &[],
            None,
            6,
            3,
        ))
        .human("2 doctests, 1 unique, 0 functions, 0 unique functions, 1 duplicated groups\n\ndoctest group g1, 2 copies\n- src/lib.rs:3  app::parse\n- src/dup.rs:3  app::parse\n\n::error file=src/lib.rs,line=3,title=echoes::app::parse duplicates the doctest at src/dup.rs:3, group 1\n")
        .exit_code(1);
    world.write_file("pkg/src/lib.rs", SRC_LIB);
    world.set_meta_root(world.workspace_path().join("pkg"));
    let exit = world.run();
    let r = world.runner();
    assert_eq!(exit, 1, "duplicate gate run exits 1");
    let scan_out = r
        .step_stdout("Scan for duplicates")
        .expect("the scan step ran");
    assert!(
        scan_out.contains("::error file=pkg/src/lib.rs,line=3,"),
        "annotation paths are rooted at the repository workspace"
    );
    assert!(
        r.summary().contains("duplicated groups"),
        "the gate summary keeps the scan text"
    );
    assert!(
        !r.summary().contains("::error"),
        "workflow commands stay out of the summary"
    );
}

#[test]
fn s03_gate_reuse_gated() {
    let mut world = World::new("s03")
        .inputs(&[
            ("context-dependency-depth", "2"),
            ("context-fail-on-reuse", "true"),
            ("context-select", "crate:app"),
        ])
        .json(report(
            &[],
            &[],
            Some(&reuse(
                &[reuse_group(
                    "R1",
                    &site("src/parser.rs", 3, 6, "app::parser::selected"),
                    &[
                        Cand::new("direct", "1.2.3", 1, "direct::helper", "reuse-api")
                            .repository("upstream/direct")
                            .api_path("direct::public::helper")
                            .source_url("https://github.com/upstream/direct/blob/abcdef1234567890abcdef1234567890abcdef12/src/lib.rs#L20-L24")
                            .build(),
                    ],
                )],
                Some(coverage(true, 1, 1, 0, &[], &[])),
            )),
            6,
            3,
        ))
        .human("1 doctest, 1 unique, 0 functions, 0 unique functions, 0 duplicated groups\n\ndependency reuse, 1 local context\n- src/parser.rs:3  app::parser::selected\n  - direct 1.2.3 (depth 1): the public item `direct::public::helper` can be reused\n")
        .exit_code(1);
    let exit = world.run();
    let r = world.runner();
    assert_eq!(exit, 1, "explicit reuse gating fails the gate run");
    let args = r.scan_args(0);
    for flag in [
        "--context-fail-on-reuse",
        "--context-select",
        "crate:app",
        "--context-dependency-depth",
        "2",
        "--context-cache-dir",
    ] {
        assert!(args.iter().any(|a| a == flag), "the scan passes {flag}");
    }
    assert!(
        !args.iter().any(|a| a == "--context-check-latest"),
        "the default latest-release check is not restated"
    );
    assert_eq!(
        r.step_outputs()
            .get("scan")
            .and_then(|o| o.get("index-ok"))
            .map(String::as_str),
        Some("true"),
        "a findings gate run with complete coverage marks the index save-worthy"
    );
    let uses = r.uses();
    assert!(
        use_by_name(uses, "Restore the dependency index").is_some(),
        "the index is restored at positive depth"
    );
    assert_eq!(
        r.step_outcome("Save the dependency index"),
        "used",
        "the valid index is saved after the findings gate failure"
    );
    let cache_dir = r
        .job_env()
        .get("echoes-context-index-dir")
        .map_or("", String::as_str);
    assert!(
        !cache_dir.is_empty() && Path::new(cache_dir).join("public").is_dir(),
        "the resolve step prepares the public cache subtree"
    );
}

#[test]
fn s04_pr_local_onlynew() {
    let base_report = report(
        &[group(
            "g1",
            "H1",
            &[doc_site("src/lib.rs", 3, 5, "app::parse")],
        )],
        &[],
        None,
        1,
        1,
    );
    let mut world = World::new("s04")
        .inputs(&[("pr-number", "7")])
        .event("pull_request")
        .git()
        .json(report(
            &[group(
                "g1",
                "H1",
                &[
                    doc_site("src/lib.rs", 3, 5, "app::parse"),
                    doc_site("src/dup.rs", 3, 5, "app::parse"),
                ],
            )],
            &[],
            None,
            6,
            3,
        ))
        .base_json(base_report)
        .changed(&["Cargo.toml", "src/lib.rs", "src/dup.rs"]);
    let exit = world.run();
    let r = world.runner();
    assert_eq!(exit, 0, "the only-new review run exits 0");
    assert_eq!(r.status(), "posted", "the review is posted");
    let comments = r.comment_posts();
    assert_eq!(
        comments.len(),
        1,
        "only the site introduced by the pull request is commented"
    );
    let params = comments[0].get("params").cloned().unwrap_or(Value::Null);
    assert_eq!(
        params.get("path").and_then(Value::as_str),
        Some("src/dup.rs"),
        "the comment targets the added copy"
    );
    assert_eq!(
        params.get("start_line").and_then(Value::as_str),
        Some("3"),
        "the comment starts on the added fence"
    );
    let reviews = r.review_posts();
    assert_eq!(reviews.len(), 1, "one review is posted");
    assert_eq!(
        reviews[0]
            .get("params")
            .and_then(|p| p.get("event"))
            .and_then(Value::as_str),
        Some("REQUEST_CHANGES"),
        "local duplicates keep the existing request-changes behavior"
    );
    let text = joined_bodies(&r.body_files());
    assert!(
        text.contains("added by this pull request"),
        "the comment names the added-copy case"
    );
    assert!(
        text.contains("```suggestion"),
        "the delete suggestion is kept for local duplicates"
    );
    let base_args = r.scan_args(1);
    let threshold = base_args.iter().position(|a| a == "--threshold");
    assert!(
        threshold.is_some() && base_args[threshold.unwrap() + 1] == "1",
        "the base scan inventories every site"
    );
    assert!(
        !base_args.iter().any(|a| a == "--context-dependency-depth"),
        "the base scan inherits no dependency depth"
    );
    assert!(
        r.summary().contains("Copies to remove"),
        "the summary lists the copies to remove"
    );
}

/// The reuse report of the four candidate shapes the rendering covers.
fn s05_report() -> Value {
    let local = site("src/parser.rs", 3, 6, "app::parser::selected");
    let url_direct = "https://github.com/upstream/direct/blob/abcdef1234567890abcdef1234567890abcdef12/src/lib.rs#L20-L24";
    let candidates = vec![
        Cand::new("direct", "1.2.3", 1, "direct::helper", "reuse-api")
            .repository("upstream/direct")
            .api_path("direct::public::helper")
            .source_url(url_direct)
            .build(),
        Cand::new("transitive", "2.0.0", 2, "transitive::helper", "upgrade-api")
            .repository("upstream/transitive")
            .api_path("transitive::helper")
            .source_url("https://github.com/upstream/transitive/blob/1234567890abcdef1234567890abcdef12345678/src/lib.rs#L20-L24")
            .existing_issue("https://github.com/upstream/transitive/issues/41", "open")
            .build(),
        Cand::new(
            "direct",
            "1.2.3",
            1,
            "direct::internal_helper",
            "request-extraction",
        )
        .access("internal-function")
        .build(),
        Cand::new("direct", "1.2.3", 1, "direct::unclear", "unresolved")
            .lookup_warning("issue lookup unavailable: HTTP 503")
            .build(),
    ];
    report(
        &[],
        &[],
        Some(&reuse(
            &[reuse_group("R1", &local, &candidates)],
            Some(coverage(
                true,
                3,
                4,
                1,
                &[],
                &["cache: restored 1 file from a stale entry"],
            )),
        )),
        6,
        3,
    )
}

#[test]
fn s05_pr_reuse_only() {
    let mut world = World::new("s05")
        .inputs(&[("pr-number", "7"), ("context-dependency-depth", "2")])
        .event("pull_request")
        .git()
        .json(s05_report())
        .base_json(report(&[], &[], None, 6, 3))
        .changed(&["Cargo.toml", "src/lib.rs", "src/parser.rs"]);
    let exit = world.run();
    let r = world.runner();
    assert_eq!(exit, 0, "advisory reuse findings do not fail the run");
    let reviews = r.review_posts();
    assert_eq!(
        reviews.len(),
        1,
        "one review is posted for the reuse findings"
    );
    assert_eq!(
        reviews[0]
            .get("params")
            .and_then(|p| p.get("event"))
            .and_then(Value::as_str),
        Some("COMMENT"),
        "dependency-only advice posts a comment review, not request changes"
    );
    assert_eq!(
        r.comment_posts().len(),
        1,
        "one neutral entry is posted on the local context"
    );
    let text = joined_bodies(&r.body_files());
    assert!(
        !text.contains("```suggestion"),
        "reuse advice carries no deletion suggestion"
    );
    assert!(
        text.contains("can be reused"),
        "the accessible API alternative is rendered"
    );
    assert!(
        text.contains("direct::public::helper"),
        "the advice names the callable public route"
    );
    assert!(
        text.contains("source (direct::helper)"),
        "the source evidence keeps the original site item"
    );
    assert!(
        text.contains("https://github.com/upstream/transitive/issues/41"),
        "the existing upstream request is linked"
    );
    assert!(
        text.contains("existing request open"),
        "the existing request state is shown"
    );
    assert!(
        text.contains("the upstream repository is not declared"),
        "the unknown repository is rendered explicitly"
    );
    assert!(
        text.contains("lookup: issue lookup unavailable: HTTP 503"),
        "the failed issue lookup is rendered explicitly"
    );
    let summary = r.summary();
    assert!(
        summary.contains("### Dependency reuse"),
        "the run summary reports the reuse groups"
    );
    let order = [
        summary.find("can be reused"),
        summary.find("newer published release"),
        summary.find("no issue link is offered"),
        summary.find("does not establish accessibility"),
    ];
    let sorted = {
        let mut s = order;
        s.sort();
        s
    };
    assert!(
        order.iter().all(Option::is_some) && order == sorted,
        "the summary keeps the report candidate order, accessible APIs first"
    );
    assert!(
        summary.contains("3 packages, 4 files indexed, 1 from cache"),
        "the summary reports the dependency coverage"
    );
    assert!(
        summary.contains("cache: restored 1 file from a stale entry"),
        "the coverage warnings are rendered"
    );
    assert!(
        !summary.contains("incomplete"),
        "a complete analysis is not flagged incomplete"
    );
}

#[test]
fn s06_pr_context_groups() {
    let mut world = World::new("s06")
        .inputs(&[("pr-number", "7"), ("context-select", "crate:app")])
        .event("pull_request")
        .git()
        .json(report(
            &[],
            &[ctx_group(
                "c9f8",
                "C1",
                &[
                    site("src/parser.rs", 3, 6, "app::parser::selected"),
                    site("src/mod.rs", 10, 13, "app::other::selected"),
                ],
            )],
            None,
            6,
            3,
        ))
        .base_json(report(&[], &[], None, 6, 3))
        .changed(&["Cargo.toml", "src/lib.rs", "src/parser.rs", "src/mod.rs"])
        .extra_base("src/mod.rs", SRC_PARSER);
    let exit = world.run();
    let r = world.runner();
    assert_eq!(exit, 0, "the context group run exits 0");
    let reviews = r.review_posts();
    assert_eq!(
        reviews
            .first()
            .and_then(|c| c.get("params"))
            .and_then(|p| p.get("event"))
            .and_then(Value::as_str),
        Some("COMMENT"),
        "context group advice posts a comment review"
    );
    assert_eq!(
        r.comment_posts().len(),
        1,
        "the kept context copy stays silent"
    );
    let text = joined_bodies(&r.body_files());
    assert!(
        text.contains("repeated in this scan"),
        "the context advice is neutral"
    );
    assert!(
        !text.contains("```suggestion"),
        "approximate contexts get no deletion suggestion"
    );
    assert!(
        !text.contains("remove") && !text.contains("delet"),
        "the context advice never suggests deletion"
    );
    let summary = r.summary();
    assert!(
        summary.contains("### Context groups"),
        "the run summary reports the context groups"
    );
    assert!(
        summary.contains("c9f8")
            && summary.contains("src/parser.rs:3")
            && summary.contains("src/mod.rs:10"),
        "the summary keeps the complete selected-corpus context group"
    );
}

#[test]
fn s07_pr_fork_403() {
    let mut world = World::new("s07")
        .inputs(&[("pr-number", "7"), ("only-new", "false")])
        .event("pull_request")
        .fork()
        .json(report(
            &[group(
                "g1",
                "H1",
                &[
                    doc_site("src/lib.rs", 3, 5, "app::parse"),
                    doc_site("src/dup.rs", 3, 5, "app::parse"),
                ],
            )],
            &[],
            None,
            6,
            3,
        ))
        .changed(&["src/lib.rs", "src/dup.rs"])
        .gh_403();
    world.write_file("src/lib.rs", SRC_LIB);
    world.write_file("src/dup.rs", SRC_DUP);
    let exit = world.run();
    let r = world.runner();
    assert_eq!(exit, 0, "a fork pull request does not fail the job");
    assert_eq!(
        r.status(),
        "skipped-readonly",
        "the review is skipped as read-only"
    );
    assert_eq!(
        r.comment_posts().len(),
        1,
        "the permission probe is attempted"
    );
    assert_eq!(
        r.review_posts().len(),
        0,
        "no review is posted without permission"
    );
    assert!(
        r.summary().contains("### echoes"),
        "the public report stays available on forks"
    );
}

#[test]
fn s08_pr_incomplete() {
    let local = site("src/parser.rs", 3, 6, "app::parser::selected");
    let mut world = World::new("s08")
        .inputs(&[("pr-number", "7"), ("context-dependency-depth", "2")])
        .event("pull_request")
        .json(report(
            &[],
            &[],
            Some(&reuse(
                &[reuse_group(
                    "R1",
                    &local,
                    &[
                        Cand::new("direct", "1.2.3", 1, "direct::helper", "reuse-api")
                            .repository("upstream/direct")
                            .source_url("https://github.com/upstream/direct/blob/abcdef1234567890abcdef1234567890abcdef12/src/lib.rs#L20-L24")
                            .build(),
                    ],
                )],
                Some(coverage(
                    false,
                    1,
                    1,
                    0,
                    &["source indexing failed: direct@1.2.3 (parse error)"],
                    &[],
                )),
            )),
            6,
            3,
        ))
        .changed(&["src/parser.rs"])
        .exit_code(2);
    let exit = world.run();
    let r = world.runner();
    assert_eq!(exit, 2, "an incomplete dependency analysis fails the run");
    let summary = r.summary();
    assert!(
        summary.contains("### Dependency reuse"),
        "the available findings are rendered before the failure"
    );
    assert!(
        summary.contains("direct@1.2.3") && summary.contains("source indexing failed"),
        "the coverage failure is rendered"
    );
    assert!(
        summary.contains("incomplete"),
        "the incomplete analysis is flagged"
    );
    assert!(
        matches!(
            r.step_outcome("Review the findings on the pull request"),
            "skipped" | "skipped-after-failure"
        ),
        "the failed scan prevents the review from posting"
    );
    assert_eq!(
        r.review_posts().len(),
        0,
        "no review is posted on incomplete analysis"
    );
}

/// The prepare fixture report and the base inventory of the two-job flow.
fn s09_fixture() -> (Value, Value) {
    let local = site("src/parser.rs", 3, 6, "app::parser::selected");
    let url_direct = "https://github.com/upstream/direct/blob/abcdef1234567890abcdef1234567890abcdef12/src/lib.rs#L20-L24";
    let rep = report(
        &[group(
            "g1",
            "H1",
            &[
                doc_site("src/lib.rs", 3, 5, "app::parse"),
                doc_site("src/dup.rs", 3, 5, "app::parse"),
            ],
        )],
        &[],
        Some(&reuse(
            &[reuse_group(
                "R1",
                &local,
                &[
                    Cand::new("direct", "1.2.3", 1, "direct::helper", "reuse-api")
                        .repository("upstream/direct")
                        .source_url(url_direct)
                        .issue_url(&issue_url("upstream", "direct", "direct::helper"))
                        .build(),
                ],
            )],
            Some(coverage(true, 1, 1, 0, &[], &[])),
        )),
        6,
        3,
    );
    let base_report = report(
        &[group(
            "g1",
            "H1",
            &[doc_site("src/lib.rs", 3, 5, "app::parse")],
        )],
        &[],
        None,
        1,
        1,
    );
    (rep, base_report)
}

/// The prepare payload files carry the prepared review.
fn s09_payload_checks(world: &World, r: &Runner) {
    let payload = world.workspace_path().join("echoes-payload");
    let meta: Value = serde_json::from_str(
        &fs::read_to_string(payload.join("review.json")).expect("read the payload metadata"),
    )
    .expect("parse the payload metadata");
    assert_eq!(
        meta.get("pr").and_then(Value::as_str),
        Some("7"),
        "the payload metadata is complete"
    );
    assert_eq!(
        meta.get("head_sha").and_then(Value::as_str).map(str::len),
        Some(40),
        "the payload metadata is complete"
    );
    assert_eq!(
        meta.get("event").and_then(Value::as_str),
        Some("REQUEST_CHANGES"),
        "the payload records the review event for the posting job"
    );
    let comments =
        fs::read_to_string(payload.join("comments.ndjson")).expect("read the payload comments");
    assert_eq!(
        comments.lines().filter(|l| !l.trim().is_empty()).count(),
        1,
        "the payload holds the local duplicate comment"
    );
    let reuse_rows =
        fs::read_to_string(payload.join("reuse.ndjson")).expect("read the payload reuse");
    let rows: Vec<&str> = reuse_rows
        .lines()
        .filter(|l| !l.trim().is_empty())
        .collect();
    assert_eq!(rows.len(), 1, "the payload serializes the reuse entry");
    assert!(
        rows.iter().any(|l| l.contains("issues/new")),
        "the reuse entry body carries the request link"
    );
    assert_eq!(r.comment_posts().len(), 0, "the prepare run posts nothing");
}

#[test]
fn s09_prepare_post_reuse() {
    let (rep, base_report) = s09_fixture();
    let mut world = World::new("s09a")
        .inputs(&[
            ("pr-number", "7"),
            ("mode", "prepare"),
            ("payload", "echoes-payload"),
            ("context-dependency-depth", "2"),
        ])
        .event("pull_request")
        .git()
        .json(rep)
        .base_json(base_report)
        .changed(&["Cargo.toml", "src/lib.rs", "src/dup.rs", "src/parser.rs"]);
    let exit = world.run();
    let r = world.runner();
    assert_eq!(exit, 0, "the prepare run exits 0");
    assert_eq!(r.status(), "prepared", "the prepare run writes the status");
    s09_payload_checks(&world, r);
    let payload_ws = world.workspace_path().to_path_buf();
    let (base, head) = world.shas();
    // The posting job runs on a fresh runner sharing the prepare workspace, so
    // the payload directory outlives the prepare job.
    let mut world2 = World::new("s09b")
        .inputs(&[
            ("pr-number", "7"),
            ("mode", "post"),
            ("payload", "echoes-payload"),
        ])
        .event("pull_request")
        .workspace(payload_ws)
        .base_sha(&base)
        .head_sha(&head);
    let exit2 = world2.run();
    let r2 = world2.runner();
    assert_eq!(exit2, 0, "the post run exits 0");
    assert_eq!(
        r2.status(),
        "posted",
        "the post run posts the prepared review"
    );
    assert_eq!(r2.scan_calls().len(), 0, "the post job rescans nothing");
    assert_eq!(
        r2.comment_posts().len(),
        2,
        "the post job posts the local comment and the reuse entry"
    );
    let reviews = r2.review_posts();
    assert_eq!(
        reviews
            .first()
            .and_then(|c| c.get("params"))
            .and_then(|p| p.get("event"))
            .and_then(Value::as_str),
        Some("REQUEST_CHANGES"),
        "the post job honors the prepared review event"
    );
}

#[test]
fn s10_dry_run_reuse() {
    let local = site("src/parser.rs", 3, 6, "app::parser::selected");
    let mut world = World::new("s10")
        .inputs(&[("dry-run", "true"), ("context-dependency-depth", "2")])
        .json(report(
            &[group(
                "g1",
                "H1",
                &[
                    doc_site("src/lib.rs", 3, 5, "app::parse"),
                    doc_site("src/dup.rs", 3, 5, "app::parse"),
                ],
            )],
            &[],
            Some(&reuse(
                &[reuse_group(
                    "R1",
                    &local,
                    &[
                        Cand::new("direct", "1.2.3", 1, "direct::helper", "reuse-api")
                            .repository("upstream/direct")
                            .source_url("https://github.com/upstream/direct/blob/abcdef1234567890abcdef1234567890abcdef12/src/lib.rs#L20-L24")
                            .build(),
                    ],
                )],
                Some(coverage(true, 1, 1, 0, &[], &[])),
            )),
            6,
            3,
        ));
    world.write_file("src/lib.rs", SRC_LIB);
    world.write_file("src/dup.rs", SRC_DUP);
    world.write_file("src/parser.rs", SRC_PARSER);
    let exit = world.run();
    let r = world.runner();
    assert_eq!(exit, 0, "the dry run exits 0");
    assert_eq!(r.status(), "dry-run", "the dry run writes its status");
    assert!(
        r.gh_calls()
            .iter()
            .all(|c| c.get("m").and_then(Value::as_str) != Some("POST")),
        "the dry run posts nothing to the API"
    );
    let body = r.body();
    assert!(
        body.contains("dry run, the review is not posted"),
        "the dry run says it posts nothing"
    );
    assert!(
        body.contains("Inline comments echoes would post"),
        "the dry run previews the local comments"
    );
    assert!(
        body.contains("Neutral reuse and context advice echoes would post"),
        "the dry run previews the neutral reuse advice"
    );
    assert!(
        body.contains("can be reused"),
        "the dry run preview names the accessible API"
    );
    assert!(
        r.summary().contains("### Dependency reuse"),
        "the summary reports the reuse groups"
    );
}

#[test]
fn s11_post_missing_payload() {
    let mut world = World::new("s11")
        .inputs(&[
            ("mode", "post"),
            ("payload", "empty-dir"),
            ("pr-number", "7"),
        ])
        .event("pull_request");
    fs::create_dir_all(world.workspace_path().join("empty-dir")).expect("make the payload dir");
    let exit = world.run();
    let r = world.runner();
    assert_eq!(exit, 1, "a missing prepared payload fails the post job");
    assert_eq!(r.status(), "failed", "the post job records the failure");
}

#[test]
fn s12_cache_trusted_save() {
    let mut world = World::new("s12")
        .inputs(&[("context-dependency-depth", "2")])
        .json(report(
            &[],
            &[],
            Some(&reuse(&[], Some(coverage(true, 1, 1, 0, &[], &[])))),
            6,
            3,
        ))
        .human("0 doctests, 0 unique, 0 functions, 0 unique functions, 0 duplicated groups\n");
    let exit = world.run();
    let r = world.runner();
    assert_eq!(exit, 0, "the trusted run exits 0");
    let uses = r.uses();
    let restore = use_by_name(uses, "Restore the dependency index");
    let save = use_by_name(uses, "Save the dependency index");
    assert!(restore.is_some(), "the trusted run restores the index");
    let expected = format!("actions/cache/restore@{CACHE_SHA}");
    assert_eq!(
        restore.map(|u| u.uses.as_str()),
        Some(expected.as_str()),
        "the restore uses the SHA-pinned cache action"
    );
    assert!(
        save.is_some(),
        "the trusted successful indexing saves the index"
    );
    let expected = format!("actions/cache/save@{CACHE_SHA}");
    assert_eq!(
        save.map(|u| u.uses.as_str()),
        Some(expected.as_str()),
        "the save uses the SHA-pinned save action"
    );
    let cache_dir = r
        .job_env()
        .get("echoes-context-index-dir")
        .map_or("", String::as_str);
    assert!(
        !cache_dir.is_empty() && Path::new(cache_dir).join("public").is_dir(),
        "the public subtree is prepared"
    );
    if let (Some(restore), Some(save)) = (restore, save) {
        let public = format!("{cache_dir}/public");
        assert_eq!(
            restore.with.get("path").map(String::as_str),
            Some(public.as_str()),
            "the cache targets the public subtree only"
        );
        assert_eq!(
            save.with.get("path").map(String::as_str),
            Some(public.as_str()),
            "the save targets the public subtree only"
        );
        let restore_key = format!("{CACHE_KEY_PREFIX}any-v1");
        assert_eq!(
            restore.with.get("key").map(String::as_str),
            Some(restore_key.as_str()),
            "the restore key names the platform and target"
        );
        assert!(
            save.with
                .get("key")
                .map(String::as_str)
                .is_some_and(|k| k.starts_with(restore_key.as_str())),
            "the save key extends the restore key with a per-run generation"
        );
        assert_ne!(
            save.with.get("key"),
            restore.with.get("key"),
            "the save key is unique so a new index generation is stored"
        );
        assert!(
            save.with
                .get("key")
                .map(String::as_str)
                .is_some_and(|k| k.starts_with(CACHE_KEY_PREFIX)),
            "the save key stays prefix-compatible with the restore fallback"
        );
        assert!(
            restore
                .with
                .get("restore-keys")
                .map(String::as_str)
                .is_some_and(|k| k.starts_with(CACHE_KEY_PREFIX)),
            "the restore falls back to the platform prefix"
        );
    }
    let args = r.scan_args(0);
    let pos = args.iter().position(|a| a == "--context-cache-dir");
    assert!(
        pos.is_some() && args[pos.unwrap() + 1] == cache_dir,
        "the scan uses the shared cache directory"
    );
}

#[test]
fn s13a_cache_fork_no_save() {
    let mut world = World::new("s13a")
        .inputs(&[("context-dependency-depth", "2")])
        .event("pull_request")
        .fork()
        .json(report(
            &[],
            &[],
            Some(&reuse(&[], Some(coverage(true, 0, 0, 0, &[], &[])))),
            6,
            3,
        ))
        .human("0 doctests, 0 unique, 0 functions, 0 unique functions, 0 duplicated groups\n");
    let exit = world.run();
    let r = world.runner();
    assert_eq!(exit, 0, "the fork run exits 0");
    let uses = r.uses();
    assert!(
        use_by_name(uses, "Restore the dependency index").is_some(),
        "a fork run may restore the index"
    );
    assert!(
        use_by_name(uses, "Save the dependency index").is_none(),
        "a fork run never uploads the index"
    );
}

#[test]
fn s13b_cache_depth_zero() {
    let mut world = World::new("s13b")
        .json(report(&[], &[], None, 6, 3))
        .human("0 doctests, 0 unique, 0 functions, 0 unique functions, 0 duplicated groups\n");
    let exit = world.run();
    let r = world.runner();
    assert_eq!(exit, 0, "the depth-zero run exits 0");
    assert_eq!(r.uses().len(), 0, "depth zero touches no cache step");
    let args = r.scan_args(0);
    assert!(
        !args.iter().any(|a| a == "--context-dependency-depth")
            && !args.iter().any(|a| a == "--context-cache-dir"),
        "depth zero passes no dependency flags"
    );
}

#[test]
fn s14_escape_untrusted() {
    let local = site("src/parser.rs", 3, 6, "app::parser::selected");
    let evil = r#"weird:item, 50% $(touch "$RUNNER_TEMP/pwned") `backtick` "quoted""#;
    let mut world = World::new("s14")
        .inputs(&[("pr-number", "7"), ("context-dependency-depth", "2")])
        .event("pull_request")
        .git()
        .json(report(
            &[],
            &[],
            Some(&reuse(
                &[reuse_group(
                    "R1",
                    &local,
                    &[
                        Cand::new("evil-pkg", "1.2.3", 1, evil, "reuse-api")
                            .repository("upstream/evil-pkg")
                            .source_url("https://github.com/upstream/evil-pkg/blob/abcdef1234567890abcdef1234567890abcdef12/src/lib.rs#L20-L24")
                            .build(),
                    ],
                )],
                Some(coverage(true, 1, 1, 0, &[], &[])),
            )),
            6,
            3,
        ))
        .changed(&["src/parser.rs"]);
    let exit = world.run();
    let r = world.runner();
    assert_eq!(exit, 0, "the untrusted-string run exits 0");
    let summary = r.summary();
    assert!(
        summary.contains("$(touch"),
        "untrusted strings render literally in the summary"
    );
    assert!(
        summary.contains("`backtick`"),
        "backticks render literally in the summary"
    );
    let text = joined_bodies(&r.body_files());
    assert!(
        text.contains("$(touch"),
        "untrusted strings render literally in the review body"
    );
    assert!(
        !Path::new(r.run_temp()).join("pwned").exists(),
        "untrusted strings are never evaluated"
    );
}

#[test]
fn s15_prepare_post_reuse_only() {
    let local = site("src/parser.rs", 3, 6, "app::parser::selected");
    let mut world = World::new("s15a")
        .inputs(&[
            ("pr-number", "7"),
            ("mode", "prepare"),
            ("payload", "echoes-payload"),
            ("context-dependency-depth", "2"),
        ])
        .event("pull_request")
        .git()
        .json(report(
            &[],
            &[],
            Some(&reuse(
                &[reuse_group(
                    "R1",
                    &local,
                    &[
                        Cand::new("direct", "1.2.3", 1, "direct::helper", "reuse-api")
                            .repository("upstream/direct")
                            .source_url("https://github.com/upstream/direct/blob/abcdef1234567890abcdef1234567890abcdef12/src/lib.rs#L20-L24")
                            .issue_url(&issue_url("upstream", "direct", "direct::helper"))
                            .build(),
                    ],
                )],
                Some(coverage(true, 1, 1, 0, &[], &[])),
            )),
            6,
            3,
        ))
        .base_json(report(&[], &[], None, 6, 3))
        .changed(&["Cargo.toml", "src/lib.rs", "src/parser.rs"]);
    let exit = world.run();

    assert_eq!(exit, 0, "the reuse-only prepare run exits 0");
    let meta: Value = serde_json::from_str(
        &fs::read_to_string(world.workspace_path().join("echoes-payload/review.json"))
            .expect("read the payload metadata"),
    )
    .expect("parse the payload metadata");
    assert_eq!(
        meta.get("event").and_then(Value::as_str),
        Some("COMMENT"),
        "a reuse-only payload records a comment review, not request changes"
    );
    // The posting job shares the prepare workspace so the payload survives it.
    let payload_ws = world.workspace_path().to_path_buf();
    let (base, head) = world.shas();
    let mut world2 = World::new("s15b")
        .inputs(&[
            ("pr-number", "7"),
            ("mode", "post"),
            ("payload", "echoes-payload"),
        ])
        .event("pull_request")
        .workspace(payload_ws)
        .base_sha(&base)
        .head_sha(&head);
    let exit2 = world2.run();
    let r2 = world2.runner();
    assert_eq!(exit2, 0, "the reuse-only post run exits 0");
    let reviews = r2.review_posts();
    assert_eq!(
        reviews
            .first()
            .and_then(|c| c.get("params"))
            .and_then(|p| p.get("event"))
            .and_then(Value::as_str),
        Some("COMMENT"),
        "dependency-only advice never triggers request changes through post"
    );
}
