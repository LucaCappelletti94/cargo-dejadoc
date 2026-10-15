//! Item payload projection through original helper proofs.

fn form(source: &str) -> syn_canon::CanonicalForm {
    syn_canon::canonicalize(syn::parse_str(source).unwrap())
}

fn failing(source: &str) -> syn_canon::CanonicalForm {
    syn_canon::canonicalize_failing(syn::parse_str(source).unwrap())
}

fn scheduled(left: &str, right: &str) {
    assert_eq!(form(left), form(right), "standalone helper provenance");
    assert_ne!(
        failing(left),
        failing(right),
        "the caller needs helper scheduling"
    );
}

#[test]
fn independent_calls_across_three_identical_helpers_share_one_schedule() {
    let helpers = "fn a(x:u32)->u32{x&15u32}fn b(x:u32)->u32{x&15u32}fn c(x:u32)->u32{x&15u32}";
    scheduled(
        &format!("{helpers}fn f(x:u32)->(u32,u32){{let p=a(x);let q=c(x);(p,q)}}"),
        &format!("{helpers}fn f(x:u32)->(u32,u32){{let q=c(x);let p=a(x);(p,q)}}"),
    );
}
