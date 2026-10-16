//! Atomic source and expanded-expression limits through public comparison.

fn xor_tree(leaf: &str, count: usize) -> String {
    if count == 1 {
        return leaf.to_owned();
    }
    format!(
        "({}^{})",
        xor_tree(leaf, count / 2),
        xor_tree(leaf, count - count / 2)
    )
}

fn form(source: &str) -> syn_canon::CanonicalForm {
    syn_canon::canonicalize(syn::parse_str(source).unwrap())
}

#[test]
fn expanded_node_limit_admits_the_exact_complete_region() {
    let original = format!("fn f(x:u8)->u8{{let t=x&1u8;{}}}", xor_tree("t", 4096));
    let inline = format!("fn f(x:u8)->u8{{{}}}", xor_tree("(x&1u8)", 4096));
    let left = form(&original);
    let right = form(&inline);
    assert_eq!(left, right);
    assert_eq!(left.body_units(), 16_383);
    assert_eq!(right.body_units(), 16_383);
    assert_ne!(
        syn_canon::canonicalize_failing(syn::parse_str(&original).unwrap()),
        syn_canon::canonicalize_failing(syn::parse_str(&inline).unwrap())
    );
}

#[test]
fn expanded_node_exhaustion_retains_the_complete_region() {
    let original = format!("fn f(x:u8)->u8{{let t=x&1u8;{}}}", xor_tree("t", 4097));
    let inline = format!("fn f(x:u8)->u8{{{}}}", xor_tree("(x&1u8)", 4097));
    let retained = form(&original);
    assert_ne!(retained, form(&inline));
    assert_eq!(retained.body_units(), 8_200);
}

fn shifts(leaf: &str, count: usize) -> String {
    let mut expression = leaf.to_owned();
    for _ in 0..count {
        expression = format!("({expression}>>1)");
    }
    expression
}

#[test]
fn expanded_expression_limit_keeps_every_original_binding_on_exhaustion() {
    for (count, admitted) in [(62, true), (63, false)] {
        let original = format!("fn f(x:u8)->u8{{let t=x>>1;{}}}", shifts("t", count));
        let inline = format!("fn f(x:u8)->u8{{{}}}", shifts("x", count + 1));
        let left = form(&original);
        let right = form(&inline);
        if admitted {
            assert_eq!(left, right);
            assert_eq!(left.body_units(), 127);
        } else {
            assert_ne!(left, right);
            assert_eq!(left.body_units(), 134);
        }
    }
}

#[test]
fn inherited_boolean_expansion_exhaustion_restores_every_value_binding() {
    let operands = xor_tree("t", 8190).replace('^', "&&");
    let original = format!("fn f(p:bool)->bool{{let t=p;!({operands})}}");
    let inline = format!("fn f(p:bool)->bool{{!({})}}", operands.replace('t', "p"));
    let retained = form(&original);
    assert_ne!(retained, form(&inline));
    assert_eq!(retained.body_units(), 16_385);
}

#[test]
fn final_boolean_syntax_exhaustion_retains_the_complete_value_region() {
    let operands = xor_tree("t", 5462)
        .replace('^', "&&")
        .replacen('t', "!t", 1);
    let original = format!("fn f(p:bool)->bool{{let t=p;!({operands})}}");
    let inline = format!("fn f(p:bool)->bool{{!({})}}", operands.replace('t', "p"));
    let retained = form(&original);
    assert_ne!(retained, form(&inline));
    assert_eq!(retained.body_units(), 10_930);
}
