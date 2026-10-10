//! Algebra proofs respect every lexical binding pattern.

fn source(binding: &str, result: &str) -> String {
    format!(
        "struct Scalar(u32); \
         impl core::marker::Copy for Scalar {{}} \
         impl core::clone::Clone for Scalar {{ fn clone(&self) -> Self {{ *self }} }} \
         struct Holder {{ slot: Scalar }} \
         impl core::ops::BitAnd for Scalar {{ \
             type Output = Self; \
             fn bitand(self, rhs: Self) -> Self {{ Self(self.0 + rhs.0) }} \
         }} \
         fn compute(value: u32) -> Scalar {{ {binding} {result} }}"
    )
}

#[test]
fn non_simple_bindings_mask_outer_primitive_algebra_facts() {
    for binding in [
        "let (value,) = (Scalar(1),);",
        "let value: Scalar = Scalar(1);",
        "let &(value,) = &(Scalar(1),);",
        "let other @ value = Scalar(1);",
        "let (value,): (Scalar,) = (Scalar(1),);",
        "let [value] = [Scalar(1)];",
        "let Holder { slot: value } = Holder { slot: Scalar(1) };",
    ] {
        let repeated =
            syn_canon::canonicalize(syn::parse_str(&source(binding, "value & value")).unwrap());
        let single = syn_canon::canonicalize(syn::parse_str(&source(binding, "value")).unwrap());
        assert_ne!(repeated, single, "{binding}");
    }
}

#[test]
fn a_shadowing_initializer_still_uses_the_outer_origin() {
    let first = syn_canon::canonicalize(
        syn::parse_str("fn compute(value: u32) -> u32 { let (value,) = (value & value,); value }")
            .unwrap(),
    );
    let second = syn_canon::canonicalize(
        syn::parse_str("fn renamed(input: u32) -> u32 { let (input,) = (input,); input }").unwrap(),
    );
    assert_eq!(first, second);
    assert_eq!(first.leaf_tokens(), second.leaf_tokens());
    assert_eq!(first.body_units(), second.body_units());
}
