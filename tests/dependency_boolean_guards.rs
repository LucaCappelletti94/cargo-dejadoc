//! Public guards for the boolean folds' wrappers, attributes, items and walk bounds.

use std::fmt::Write as _;

use syn::visit_mut::VisitMut as _;

fn form(source: &str) -> syn_canon::CanonicalForm {
    syn_canon::canonicalize(syn::parse_str(source).unwrap())
}

fn failing(source: &str) -> syn_canon::CanonicalForm {
    syn_canon::canonicalize_failing(syn::parse_str(source).unwrap())
}

fn same(left: &str, right: &str) {
    let a = form(left);
    let b = form(right);
    assert_eq!(a, b, "{left}\n{right}");
    assert_eq!(a.body_units(), b.body_units());
    assert_eq!(a.leaf_tokens(), b.leaf_tokens());
    assert_ne!(failing(left), failing(right));
}

fn separate(left: &str, right: &str) {
    assert_ne!(form(left), form(right), "{left}\n{right}");
    assert_ne!(failing(left), failing(right));
}

fn names(count: usize) -> Vec<String> {
    (0..count).map(|index| format!("a{index}")).collect()
}

fn params(names: &[&str]) -> String {
    names
        .iter()
        .map(|name| format!("{name}: bool"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// A left-nested `&&` chain, one binary expression per extra leaf.
fn chain(names: &[&str]) -> String {
    names.join(" && ")
}

fn deep_if(leaves: usize) -> String {
    let ns = names(leaves);
    let refs: Vec<&str> = ns.iter().map(String::as_str).collect();
    format!(
        "fn f({}) -> bool {{ if true {{ {} }} else {{ false }} }}",
        params(&refs),
        chain(&refs)
    )
}

fn deep_bare(leaves: usize) -> String {
    let ns = names(leaves);
    let refs: Vec<&str> = ns.iter().map(String::as_str).collect();
    format!("fn g({}) -> bool {{ {} }}", params(&refs), chain(&refs))
}

fn inversion(body: &str, negated: bool) -> String {
    let (condition, then, otherwise) = if negated {
        ("!p", body, "false")
    } else {
        ("p", "false", body)
    };
    format!("fn f(p:bool,q:bool)->bool{{if {condition}{{{then}}}else{{{otherwise}}}}}")
}

fn balanced(count: usize) -> String {
    if count == 1 {
        return "q".to_owned();
    }
    format!(
        "({} && {})",
        balanced(count / 2),
        balanced(count - count / 2)
    )
}

struct GroupCond {
    negated: bool,
    attributed: bool,
}

impl syn::visit_mut::VisitMut for GroupCond {
    fn visit_expr_if_mut(&mut self, r#if: &mut syn::ExprIf) {
        let inner: syn::Expr = syn::parse_str("p && q").unwrap();
        let attrs = if self.attributed {
            let attr: syn::Attribute = syn::parse_quote!(#[cfg(all())]);
            vec![attr]
        } else {
            Vec::new()
        };
        let group = syn::Expr::Group(syn::ExprGroup {
            attrs,
            group_token: syn::token::Group::default(),
            expr: Box::new(inner),
        });
        let expr = if self.negated {
            syn::Expr::Unary(syn::ExprUnary {
                attrs: Vec::new(),
                op: syn::UnOp::Not(syn::token::Not::default()),
                expr: Box::new(group),
            })
        } else {
            group
        };
        *r#if.cond = expr;
    }
}

fn grouped(
    negated: bool,
    attributed: bool,
) -> (syn_canon::CanonicalForm, syn_canon::CanonicalForm) {
    let make = |file: &mut syn::File| {
        GroupCond {
            negated,
            attributed,
        }
        .visit_file_mut(file);
    };
    let mut file: syn::File =
        syn::parse_str("fn f(p: bool, q: bool) -> bool { if p { true } else { false } }").unwrap();
    make(&mut file);
    let passing = syn_canon::canonicalize(file);
    let mut file: syn::File =
        syn::parse_str("fn f(p: bool, q: bool) -> bool { if p { true } else { false } }").unwrap();
    make(&mut file);
    let failing = syn_canon::canonicalize_failing(file);
    (passing, failing)
}

fn same_forms(left: (syn_canon::CanonicalForm, syn_canon::CanonicalForm), right: &str) {
    let (a, a_failing) = left;
    let b = form(right);
    assert_eq!(a, b, "{right}");
    assert_eq!(a.body_units(), b.body_units());
    assert_eq!(a.leaf_tokens(), b.leaf_tokens());
    assert_ne!(a_failing, failing(right));
}

fn separate_forms(
    left: &(syn_canon::CanonicalForm, syn_canon::CanonicalForm),
    right: &(syn_canon::CanonicalForm, syn_canon::CanonicalForm),
) {
    assert_ne!(left.0, right.0);
    assert_ne!(left.1, right.1);
}

#[test]
fn literal_negation_folds_primitives_and_compositions() {
    same("fn f() -> bool { !true }", "fn g() -> bool { false }");
    same("fn f() -> bool { !false }", "fn g() -> bool { true }");
    same(
        "fn f(p: bool) -> bool { !true && p }",
        "fn g(p: bool) -> bool { false && p }",
    );
    same(
        "fn f(p: bool) -> bool { !(true && p) }",
        "fn g(p: bool) -> bool { false || !p }",
    );
    separate("fn f() -> bool { !true }", "fn g() -> bool { true }");
    separate(
        "fn f(p: bool) -> bool { !true && p }",
        "fn g(p: bool) -> bool { true && p }",
    );
}

#[test]
fn grouped_total_conditions_fold_like_source_parens() {
    same(
        "fn f(p: bool, q: bool) -> bool { if (p && q) { true } else { false } }",
        "fn g(x: bool, y: bool) -> bool { x && y }",
    );
    same_forms(
        grouped(false, false),
        "fn g(x: bool, y: bool) -> bool { x && y }",
    );
    same_forms(
        grouped(true, false),
        "fn g(x: bool, y: bool) -> bool { !x || !y }",
    );
    separate_forms(&grouped(true, true), &grouped(true, false));
}

#[test]
fn active_attributes_keep_the_condition_retained() {
    separate(
        "fn f(p: bool) -> bool { #[cfg(all())] if p { true } else { false } }",
        "fn g(x: bool) -> bool { x }",
    );
    separate(
        "fn f(p: bool) -> bool { if !p { #[cfg(all())] true } else { false } }",
        "fn g(x: bool) -> bool { if x { false } else { true } }",
    );
    separate(
        "fn f(p: bool) -> u8 { if !(#[cfg(all())] p) { 1u8 } else { 2u8 } }",
        "fn g(x: bool) -> u8 { if x { 2u8 } else { 1u8 } }",
    );
    separate(
        "fn f(p: bool) -> bool { if p { #[cfg(all())] true } else { false } }",
        "fn g(x: bool) -> bool { x }",
    );
}

#[test]
fn branch_local_items_keep_the_condition_retained() {
    separate(
        "fn f(p: bool) -> bool { if p { fn inner(q: bool) -> bool { q } inner(p) } else { false } }",
        "fn g(x: bool) -> bool { x }",
    );
    let left = "fn f(p: bool) -> bool { if p { fn inner(q: bool) -> bool { q } inner(p) } else { false } }";
    let right = "fn g(p: bool) -> bool { if p { fn local(r: bool) -> bool { r } local(p) } else { false } }";
    assert_eq!(form(left), form(right));
    assert_eq!(failing(left), failing(right));
    separate(
        "fn f(p: bool) -> bool { if p { const LIM: i32 = 3; p } else { false } }",
        "fn g(x: bool) -> bool { x }",
    );
}

#[test]
fn branch_block_depth_bounds_retain_the_complete_conditional() {
    for (nested, leaves, admitted) in [
        (63, 1, true),
        (64, 1, false),
        (63, 64, true),
        (63, 65, false),
    ] {
        let mut body = vec!["q"; leaves].join(" && ");
        for _ in 0..nested {
            body = format!("{{{body}}}");
        }
        let original = inversion(&body, true);
        let oriented = inversion(&body, false);
        if admitted {
            same(&original, &oriented);
        } else {
            separate(&original, &oriented);
        }
    }
}

#[test]
fn branch_node_bounds_retain_the_complete_conditional() {
    let statements = |count| format!("{}q", "0u8;".repeat(count));
    let items = |count| {
        let mut body = String::new();
        for index in 0..count {
            write!(body, "const C{index}:u8=0;").unwrap();
        }
        body.push('q');
        body
    };
    let expressions = |count| format!("{}const C:u8=0;q", "0u8;".repeat(count));
    for body in [statements(8190), items(5460), expressions(8188)] {
        same(&inversion(&body, true), &inversion(&body, false));
    }
    for body in [statements(8193), items(5462), expressions(8191)] {
        separate(&inversion(&body, true), &inversion(&body, false));
    }
}

#[test]
fn deep_boolean_tails_fold_up_to_the_expression_bound() {
    same(&deep_if(64), &deep_bare(64));
    separate(&deep_if(65), &deep_bare(65));
}

#[test]
fn branch_expression_depth_bounds_retain_the_complete_conditional() {
    let body = |count| vec!["q"; count].join(" && ");
    same(&inversion(&body(64), true), &inversion(&body(64), false));
    separate(&inversion(&body(65), true), &inversion(&body(65), false));
}

#[test]
fn wide_boolean_conditions_retain_the_complete_conditional_at_the_node_bound() {
    for (count, admitted) in [(8191, true), (8192, false)] {
        let condition = balanced(count);
        let original = format!("fn f(q:bool)->bool{{if {condition}{{true}}else{{false}}}}");
        let folded = format!("fn g(q:bool)->bool{{{condition}}}");
        if admitted {
            same(&original, &folded);
        } else {
            separate(&original, &folded);
        }
    }
}

#[test]
fn oriented_comparisons_reorient_only_complementary_conditions() {
    same(
        "fn f(p: i32, q: i32) -> bool { if q > p { true } else { false } }",
        "fn g(x: i32, y: i32) -> bool { x < y }",
    );
    same(
        "fn f(p: i32, q: i32) -> bool { if q >= p { true } else { false } }",
        "fn g(x: i32, y: i32) -> bool { x <= y }",
    );
    separate(
        "fn f(p: i32, q: i32) -> u32 { if q > p { 1u32 } else { 2u32 } }",
        "fn g(p: i32, q: i32) -> u32 { if p < q { 1u32 } else { 2u32 } }",
    );
}

#[test]
fn negated_future_reads_share_their_contexts() {
    same(
        "fn f(p: bool, q: bool) -> (bool, bool) { let a = p; let b = q; println!(\"barrier\"); let c = !(a && b); let d = q; (c, d) }",
        "fn g(x: bool, y: bool) -> (bool, bool) { let a = x; let b = y; println!(\"barrier\"); let c = !a || !b; let d = y; (c, d) }",
    );
    same(
        "fn f(p: bool, q: bool) -> (bool, bool) { let a = p; let b = q; println!(\"barrier\"); let c = !!a; let d = b; (c, d) }",
        "fn g(p: bool, q: bool) -> (bool, bool) { let a = p; let b = q; println!(\"barrier\"); let c = a; let d = b; (c, d) }",
    );
    separate(
        "fn f(p: bool, q: bool) -> (bool, bool) { let a = p; let b = q; println!(\"barrier\"); let c = !a; let d = b; (c, d) }",
        "fn g(p: bool, q: bool) -> (bool, bool) { let a = p; let b = q; println!(\"barrier\"); let c = a; let d = b; (c, d) }",
    );
}
