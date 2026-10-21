use echoes::Group;

/// Every site of the function groups is public and lives in `src/lib.rs`.
pub(super) fn assert_public_sites(function_groups: &[&Group]) {
    for group in function_groups {
        assert!(group.remedy.is_some());
        assert!(group.sites.iter().all(|site| {
            site.file == "src/lib.rs" && site.line > 0 && site.end.is_some() && site.public
        }));
    }
}
