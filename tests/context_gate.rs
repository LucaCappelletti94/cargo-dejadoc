//! Dependency findings and incomplete analysis through the public exit policy.

#![cfg(feature = "std")]

use echoes::{
    ContextAccess, ContextKind, ContextSite, DependencyCandidate, DependencyCoverage, ReuseAction,
    ReuseGroup, ReuseReport,
};
use std::process::ExitCode;

fn report() -> echoes::Report {
    let mut report = echoes::group(&[], 2, 0);
    report.reuse = Some(ReuseReport {
        groups: vec![ReuseGroup {
            hash: "match".into(),
            tokens: 40,
            local: ContextSite {
                file: "src/lib.rs".into(),
                line: 2,
                column: 1,
                end: 4,
                end_column: 2,
                item: "consumer::local".into(),
                kind: ContextKind::FunctionBody,
            },
            candidates: vec![DependencyCandidate {
                package: "helper".into(),
                version: "1.0.0".into(),
                source: "path".into(),
                depth: 1,
                site: ContextSite {
                    file: "src/lib.rs".into(),
                    line: 7,
                    column: 1,
                    end: 9,
                    end_column: 2,
                    item: "helper::available".into(),
                    kind: ContextKind::FunctionBody,
                },
                access: ContextAccess::PublicFunction,
                api_path: Some("helper::available".into()),
                conditions: Vec::new(),
                notes: Vec::new(),
                action: ReuseAction::ReuseApi,
                repository: None,
                source_url: None,
                issue_url: None,
                existing_issue: None,
                lookup_warning: None,
            }],
        }],
        coverage: DependencyCoverage::default(),
        fail_on_findings: false,
    });
    report
}

#[test]
fn dependency_findings_are_advisory_until_explicitly_gated() {
    let mut report = report();
    assert_eq!(echoes::exit_code(&report, false), ExitCode::SUCCESS);
    report.reuse.as_mut().unwrap().fail_on_findings = true;
    assert_eq!(echoes::exit_code(&report, false), ExitCode::FAILURE);
    assert_eq!(echoes::exit_code(&report, true), ExitCode::SUCCESS);
}

#[test]
fn incomplete_dependency_analysis_fails_even_under_no_fail() {
    let mut report = report();
    let coverage = &mut report.reuse.as_mut().unwrap().coverage;
    coverage.complete = false;
    coverage
        .failures
        .push("dependency source unavailable".into());
    assert_eq!(echoes::exit_code(&report, false), ExitCode::from(2));
    assert_eq!(echoes::exit_code(&report, true), ExitCode::from(2));
}

#[test]
fn reuse_gate_does_not_change_existing_local_duplicate_policy() {
    let mut report = report();
    let local = report.reuse.as_ref().unwrap().groups[0].local.clone();
    let mut other = local.clone();
    other.line = 12;
    other.end = 14;
    other.item = "consumer::other".into();
    report.context_groups.push(echoes::ContextGroup {
        id: "local".into(),
        hash: "local".into(),
        tokens: 40,
        sites: vec![local, other],
    });
    assert_eq!(echoes::exit_code(&report, false), ExitCode::FAILURE);
    assert_eq!(echoes::exit_code(&report, true), ExitCode::SUCCESS);
}
