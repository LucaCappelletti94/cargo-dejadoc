use dejadoc::group;

use crate::{group_members, site_src, support};

/// The two passing sites of one case group together, apart from the compile-failing copies.
pub(super) fn assert_fixture_group(case: &support::Case) {
    let sites = [
        site_src::site("pass_a", &case.a, &[]),
        site_src::site("pass_b", &case.b, &[]),
        site_src::site("fail_a", &case.a, &["compile_fail"]),
        site_src::site("fail_b", &case.b, &["compile_fail"]),
    ];
    let report = group(&sites, 2, 0);
    assert_eq!(
        group_members::members(report.groups.iter()),
        [vec!["pass_a", "pass_b"]],
        "{}",
        case.id
    );
    assert!(
        report.groups[0]
            .sites
            .iter()
            .all(|member| { member.line == 7 && member.end == Some(11) && member.info.is_empty() })
    );
}
