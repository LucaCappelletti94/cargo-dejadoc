//! Public relations for independently executed primitive algebra witnesses.

#[path = "support/witnesses.rs"]
mod witnesses;

use witnesses::relations;

#[test]
fn native_algebra_sources_preserve_their_public_relations() {
    let mut cases: Vec<witnesses::Case> =
        serde_json::from_str(include_str!("fixtures/dependency/algebra_cases.json")).unwrap();
    cases.extend(
        serde_json::from_str::<Vec<witnesses::Case>>(include_str!(
            "fixtures/dependency/algebra_signed_cases.json"
        ))
        .unwrap(),
    );
    relations(&cases);
}
