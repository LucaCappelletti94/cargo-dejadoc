//! Canonical grouping across typed schedules and source-site metadata.

#![no_main]

use dejadoc::{DocTest, group};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let data = data.to_vec();
    dejadoc_fuzz::on_large_stack(move || {
        dejadoc_fuzz::check_dependency_schedules(&data);
        dejadoc_fuzz::check_boolean_schedules(&data);
        let Ok(code) = std::str::from_utf8(&data) else {
            return;
        };
        let site = |item: &str| DocTest {
            file: format!("src/{item}.rs"),
            line: 1,
            end: None,
            item: format!("c::{item}"),
            info: Vec::new(),
            code: code.into(),
            allow: false,
            self_type: None,
            public: false,
        };
        let report = group(&[site("first"), site("second")], 1, 0);
        let mut members: Vec<_> = report
            .groups
            .iter()
            .map(|group| {
                let mut sites: Vec<_> = group.sites.iter().map(|site| site.item.as_str()).collect();
                sites.sort_unstable();
                sites
            })
            .collect();
        members.sort_unstable();
        assert_eq!(members, [vec!["c::first", "c::second"]]);
    });
});
