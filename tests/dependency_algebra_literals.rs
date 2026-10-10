//! Signed literal algebra composes with checked constant evaluation.

#[test]
fn signed_literal_subsets_converge_across_association() {
    for ty in ["i8", "i16", "i32", "i64", "i128"] {
        for op in ['&', '|', '^'] {
            let left = format!("fn f(a:{ty})->{ty}{{(a {op} -1{ty}) {op} 5{ty}}}");
            let right = format!("fn g(x:{ty})->{ty}{{x {op} (-1{ty} {op} 5{ty})}}");
            let a = syn_canon::canonicalize(syn::parse_str(&left).unwrap());
            let b = syn_canon::canonicalize(syn::parse_str(&right).unwrap());
            assert_eq!(a, b, "{ty} {op}");
            assert_eq!(a.leaf_tokens(), b.leaf_tokens());
            assert_eq!(a.body_units(), b.body_units());
            assert_ne!(
                syn_canon::canonicalize_failing(syn::parse_str(&left).unwrap()),
                syn_canon::canonicalize_failing(syn::parse_str(&right).unwrap()),
                "{ty} {op}"
            );
        }
    }
}

#[test]
fn signed_minimum_literals_share_their_checked_value_identity() {
    for (ty, minimum) in [
        ("i8", "128"),
        ("i16", "32768"),
        ("i32", "2147483648"),
        ("i64", "9223372036854775808"),
        ("i128", "170141183460469231731687303715884105728"),
    ] {
        let left = format!("fn f(a:{ty})->{ty}{{(a | -{minimum}{ty}) | (a | -{minimum}{ty})}}");
        let right = format!("fn g(x:{ty})->{ty}{{-{minimum}{ty} | x}}");
        let a = syn_canon::canonicalize(syn::parse_str(&left).unwrap());
        let b = syn_canon::canonicalize(syn::parse_str(&right).unwrap());
        assert_eq!(a, b, "{ty}");
        assert_eq!(a.leaf_tokens(), b.leaf_tokens());
        assert_eq!(a.body_units(), b.body_units());
    }
}
