use serde::Deserialize;
use std::sync::LazyLock;

#[derive(Deserialize)]
pub struct Case {
    pub id: String,
    pub family: String,
    pub a: String,
    pub b: String,
    pub expected: String,
}

pub fn cases() -> &'static [Case] {
    static CASES: LazyLock<Vec<Case>> = LazyLock::new(|| {
        serde_json::from_str(include_str!("../fixtures/dependency/cases.json")).unwrap()
    });
    &CASES
}
