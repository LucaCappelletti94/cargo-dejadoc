use dejadoc::DocTest;

/// A synthetic doctest site spanning the whole `<item>.rs` file.
pub(super) fn site(item: &str, code: &str, info: &[&str]) -> DocTest {
    DocTest {
        file: format!("{item}.rs"),
        line: 1,
        end: None,
        item: item.to_owned(),
        info: info.iter().map(|tag| (*tag).to_owned()).collect(),
        code: code.to_owned(),
        allow: false,
        self_type: None,
        public: false,
    }
}
