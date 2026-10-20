//! Public grouping boundaries for checked fixed-width constants.

#[path = "support/dependency.rs"]
mod support;

#[path = "support/group_members.rs"]
mod group_members;
#[path = "support/site_file.rs"]
mod site_file;

use group_members::members;
use site_file::site;

use dejadoc::{DocTest, group};

fn a09() -> &'static support::Case {
    support::cases()
        .iter()
        .find(|case| case.id == "A09_constant_eval")
        .unwrap()
}

fn neighbours() -> (DocTest, DocTest) {
    (
        site(
            "value",
            "fn f(input: u32) -> u32 { let n = 6u32; n ^ input } fn main() { for input in [0, 1, 15] { println!(\"{:?}\", f(input)); } }",
            &[],
        ),
        site(
            "width",
            "fn f(input: u16) -> u16 { let n = 2u16 + 3u16; n ^ input } fn main() { for input in [0, 1, 15] { println!(\"{:?}\", f(input)); } }",
            &[],
        ),
    )
}

#[test]
fn constant_grouping_joins_the_folded_a09_pair() {
    for case in support::cases()
        .iter()
        .filter(|case| case.family == "constant" && case.expected == "same")
    {
        let sites = [site("folded", &case.a, &[]), site("literal", &case.b, &[])];
        assert_eq!(
            members(group(&sites, 1, 0).groups.iter()),
            vec![vec!["folded", "literal"]],
            "{}",
            case.id
        );
    }
}

#[test]
fn constant_grouping_keeps_neighbours_outside_the_group() {
    let case = a09();
    let (value, width) = neighbours();
    let sites = [
        site("folded", &case.a, &[]),
        site("literal", &case.b, &[]),
        value,
        width,
    ];
    assert_eq!(
        members(group(&sites, 1, 0).groups.iter()),
        vec![vec!["folded", "literal"], vec!["value"], vec!["width"]]
    );
}

#[test]
fn constant_grouping_separates_compile_fail_forms() {
    let case = a09();
    let sites = [
        site("pass_folded", &case.a, &[]),
        site("pass_literal", &case.b, &[]),
        site("fail_folded", &case.a, &["compile_fail"]),
        site("fail_literal", &case.b, &["compile_fail"]),
    ];
    assert_eq!(
        members(group(&sites, 2, 0).groups.iter()),
        vec![vec!["pass_folded", "pass_literal"]]
    );
}

#[test]
fn constant_grouping_threshold_and_canonical_size_boundary() {
    let case = a09();
    let sites = [site("folded", &case.a, &[]), site("literal", &case.b, &[])];
    assert_eq!(
        members(group(&sites, 1, 0).groups.iter()),
        vec![vec!["folded", "literal"]]
    );
    assert_eq!(
        members(group(&sites, 2, 0).groups.iter()),
        vec![vec!["folded", "literal"]]
    );
    assert_eq!(
        members(group(&sites, 3, 0).groups.iter()),
        Vec::<Vec<&str>>::new()
    );
    let size = group(&sites, 2, 0).groups[0].tokens;
    assert_eq!(
        members(group(&sites, 2, size).groups.iter()),
        vec![vec!["folded", "literal"]]
    );
    assert_eq!(
        members(group(&sites, 2, size + 1).groups.iter()),
        Vec::<Vec<&str>>::new()
    );
    let unfolded = syn_canon::canonicalize(syn::parse_str(&case.a).unwrap());
    let collapsed = syn_canon::canonicalize(syn::parse_str(&case.b).unwrap());
    assert_eq!(unfolded, collapsed);
    assert_eq!(collapsed.body_units(), unfolded.body_units());
    let scalar = syn_canon::canonicalize(syn::parse_str("fn f() -> u32 { 2u32 + 3u32 }").unwrap());
    assert_eq!(scalar.body_units(), 1);
}

#[test]
fn constant_grouping_identity_is_independent_of_site_enumeration() {
    let case = a09();
    let (value, width) = neighbours();
    let sites = [
        site("folded", &case.a, &[]),
        site("literal", &case.b, &[]),
        value,
        width,
    ];
    let reversed: Vec<DocTest> = sites.iter().rev().cloned().collect();
    assert_eq!(
        members(group(&sites, 1, 0).groups.iter()),
        members(group(&reversed, 1, 0).groups.iter())
    );
}
