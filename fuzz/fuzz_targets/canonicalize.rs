//! A doctest body through `group`. Two copies at different sites must land
//! in one group, so the canonical form is the same each time it is computed.

#![no_main]

use dejadoc::{DocTest, group};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(code) = std::str::from_utf8(data) else {
        return;
    };
    let site = |line| DocTest {
        file: "src/lib.rs".into(),
        line,
        end: None,
        item: "c::f".into(),
        info: Vec::new(),
        code: code.into(),
        allow: false,
        self_type: None,
        public: false,
    };
    let report = group(&[site(1), site(2)], 1, 0);
    assert_eq!(report.total, 2);
    assert_eq!(
        report.groups.len(),
        1,
        "two copies of one body canonicalized apart"
    );
});
