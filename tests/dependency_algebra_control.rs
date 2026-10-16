//! Algebra eligibility follows execution regions and independent item bodies.

fn form(source: &str) -> syn_canon::CanonicalForm {
    syn_canon::canonicalize(syn::parse_str(source).unwrap())
}

#[test]
fn unreachable_nested_expressions_retain_their_operations() {
    let repeated = "fn f(a:u32)->u32{return 0u32;{let seed=a;seed & seed}}";
    let single = "fn g(x:u32)->u32{return 0u32;{let seed=x;seed}}";
    assert_ne!(form(repeated), form(single));
}

#[test]
fn hoisted_functions_have_independent_algebra_eligibility() {
    let repeated =
        "fn f(a:u32)->u32{helper(a);return 0u32;fn helper(value:u32)->u32{value & value}}";
    let single = "fn g(x:u32)->u32{helper(x);return 0u32;fn helper(input:u32)->u32{input}}";
    let first = form(repeated);
    let second = form(single);
    assert_eq!(first, second);
    assert_eq!(first.leaf_tokens(), second.leaf_tokens());
    assert_eq!(first.body_units(), second.body_units());
}
