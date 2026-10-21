use echoes::DocTest;

/// A synthetic function site at `src/<item>.rs`, lines 7 through 11.
pub(super) fn site(item: &str, code: &str, info: &[&str]) -> DocTest {
    DocTest {
        file: format!("src/{item}.rs"),
        line: 7,
        end: Some(11),
        item: item.to_owned(),
        info: info.iter().map(|tag| (*tag).to_owned()).collect(),
        code: code.to_owned(),
        allow: false,
        self_type: None,
        public: false,
    }
}
