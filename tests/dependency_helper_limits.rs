//! Atomic helper summary, source, labeling and final-syntax boundaries.

use std::fmt::Write as _;

#[derive(Default)]
struct DeclarationNodes {
    stmt: usize,
    expr: usize,
    item: usize,
}

impl syn::visit::Visit<'_> for DeclarationNodes {
    fn visit_stmt(&mut self, stmt: &syn::Stmt) {
        self.stmt += 1;
        syn::visit::visit_stmt(self, stmt);
    }
    fn visit_expr(&mut self, expr: &syn::Expr) {
        self.expr += 1;
        syn::visit::visit_expr(self, expr);
    }
    fn visit_item(&mut self, item: &syn::Item) {
        self.item += 1;
        syn::visit::visit_item(self, item);
    }
}

fn declaration_nodes(source: &str, names: &[&str]) -> usize {
    let file: syn::File = syn::parse_str(source).unwrap();
    names
        .iter()
        .map(|name| {
            let function = file
                .items
                .iter()
                .find_map(|item| match item {
                    syn::Item::Fn(function) if function.sig.ident == *name => Some(function),
                    _ => None,
                })
                .unwrap();
            let mut count = DeclarationNodes::default();
            syn::visit::visit_block(&mut count, &function.block);
            1 + count.stmt + count.expr + count.item
        })
        .sum()
}

#[derive(Default)]
struct ExprDepth {
    depth: usize,
    best: usize,
}

impl syn::visit::Visit<'_> for ExprDepth {
    fn visit_expr(&mut self, expr: &syn::Expr) {
        self.depth += 1;
        self.best = self.best.max(self.depth);
        syn::visit::visit_expr(self, expr);
        self.depth -= 1;
    }
}

fn expression_depth(source: &str, name: &str) -> usize {
    let file: syn::File = syn::parse_str(source).unwrap();
    let function = file
        .items
        .iter()
        .find_map(|item| match item {
            syn::Item::Fn(function) if function.sig.ident == name => Some(function),
            _ => None,
        })
        .unwrap();
    let mut count = ExprDepth::default();
    syn::visit::visit_block(&mut count, &function.block);
    count.best
}

fn form(source: &str) -> syn_canon::CanonicalForm {
    syn_canon::canonicalize(syn::parse_str(source).unwrap())
}

fn summary_endpoint(extra: usize, reversed: bool, tail: &str) -> String {
    let mut body = "true;".repeat(8185);
    match extra {
        0 => {}
        1 => body.replace_range(..5, "!p;"),
        2 => body.push_str("p;"),
        _ => unreachable!("`extra` is 0, 1 or 2"),
    }
    let statements = if reversed {
        "let b=k(!p);let a=h(p);"
    } else {
        "let a=h(p);let b=k(!p);"
    };
    format!(
        "fn f(p:bool)->(bool,bool){{{statements}(a,b)}}fn h(p:bool)->bool{{{body}g(p);{tail}}}fn g(p:bool)->bool{{!p}}fn k(p:bool)->bool{{p}}"
    )
}

#[test]
fn the_summary_domain_at_the_node_cap_still_schedules() {
    let a = summary_endpoint(0, false, "p");
    let b = summary_endpoint(0, true, "p");
    assert_eq!(declaration_nodes(&a, &["h", "g", "k"]), 16_384);
    let first = form(&a);
    assert_eq!(first, form(&b));
    assert_ne!(first, form(&summary_endpoint(0, false, "false")));
}

#[test]
fn a_summary_domain_one_node_over_the_cap_retains_source_order() {
    let a = summary_endpoint(1, false, "p");
    let b = summary_endpoint(1, true, "p");
    assert_eq!(declaration_nodes(&a, &["h", "g", "k"]), 16_385);
    assert_ne!(form(&a), form(&b));
}

#[test]
fn a_summary_domain_two_nodes_over_the_cap_retains_source_order() {
    let a = summary_endpoint(2, false, "p");
    let b = summary_endpoint(2, true, "p");
    assert_eq!(declaration_nodes(&a, &["h", "g", "k"]), 16_386);
    assert_ne!(form(&a), form(&b));
}

#[test]
fn a_retained_region_keeps_its_dead_caller_binding() {
    let plain = summary_endpoint(1, false, "p");
    let dead = plain.replacen("let a=h(p);", "let dead=true;let a=h(p);", 1);
    let reversed = dead.replacen("let a=h(p);let b=k(!p);", "let b=k(!p);let a=h(p);", 1);
    assert_eq!(declaration_nodes(&dead, &["h", "g", "k"]), 16_385);
    let with_dead = form(&dead);
    assert_ne!(with_dead, form(&plain));
    assert_ne!(with_dead, form(&reversed));
}

#[test]
fn a_shared_helper_counts_once_in_the_summary_domain() {
    let body = "!p;".to_owned() + &"true;".repeat(8177);
    let helpers = format!(
        "fn h(p:bool)->bool{{left(p);right(p);p}}fn left(p:bool)->bool{{base(p)}}fn right(p:bool)->bool{{base(p)}}fn base(p:bool)->bool{{{body}p}}fn k(p:bool)->bool{{p}}"
    );
    let a = format!("{helpers}fn f(p:bool)->(bool,bool){{let a=h(p);let b=k(!p);(a,b)}}");
    let b = format!("{helpers}fn f(p:bool)->(bool,bool){{let b=k(!p);let a=h(p);(a,b)}}");
    assert_eq!(
        declaration_nodes(&a, &["h", "left", "right", "base", "k"]),
        16_384
    );
    assert_eq!(form(&a), form(&b));
}

#[test]
fn an_oversized_single_declaration_rejects_the_domain() {
    let helpers = format!(
        "fn big(p:bool)->bool{{{}p}}fn k(p:bool)->bool{{p}}",
        "true;".repeat(8191)
    );
    let a = format!("{helpers}fn f(p:bool)->(bool,bool){{let a=big(p);let b=k(!p);(a,b)}}");
    let b = format!("{helpers}fn f(p:bool)->(bool,bool){{let b=k(!p);let a=big(p);(a,b)}}");
    assert_eq!(declaration_nodes(&a, &["big"]), 16_385);
    assert_ne!(form(&a), form(&b));
}

#[test]
fn an_expression_chain_at_the_depth_cap_still_summarizes() {
    for (wraps, admitted) in [(10, true), (62, true), (63, false)] {
        let (a, b) = (
            format!(
                "fn h(x:u32)->u32{{x{}^1u32}} fn k(x:u32)->u32{{x>>4}} fn f(x:u32)->(u32,u32){{let a=h(x);let b=k(x^1u32);(a,b)}}",
                "^1u32".repeat(wraps)
            ),
            format!(
                "fn h(x:u32)->u32{{x{}^1u32}} fn k(x:u32)->u32{{x>>4}} fn f(x:u32)->(u32,u32){{let b=k(x^1u32);let a=h(x);(a,b)}}",
                "^1u32".repeat(wraps)
            ),
        );
        assert_eq!(
            expression_depth(&a, "h"),
            wraps + 2,
            "the chain depth is counted from the tail expression"
        );
        if admitted {
            assert_eq!(form(&a), form(&b), "the at-or-under-cap chain summarizes");
        } else {
            assert_ne!(
                form(&a),
                form(&b),
                "the over-cap chain retains source order"
            );
        }
    }
}

#[test]
fn a_lexical_nest_at_the_cap_still_summarizes() {
    for (blocks, admitted) in [(62, true), (63, true), (64, false)] {
        let inner = "{ ".repeat(blocks) + "1u32" + &"}".repeat(blocks);
        let (a, b) = (
            format!(
                "fn h()->u32{{{inner}}} fn k()->u32{{2u32}} fn f()->(u32,u32){{let a=h();let b=k();(a,b)}}"
            ),
            format!(
                "fn h()->u32{{{inner}}} fn k()->u32{{2u32}} fn f()->(u32,u32){{let b=k();let a=h();(a,b)}}"
            ),
        );
        if admitted {
            assert_eq!(form(&a), form(&b), "the at-or-under-cap nest summarizes");
        } else {
            assert_ne!(form(&a), form(&b), "the over-cap nest retains source order");
        }
    }
}

#[test]
fn exhausted_depth_summaries_keep_the_dead_caller_binding() {
    let chain = format!(
        "fn h(x:u32)->u32{{x{}^1u32}} fn k(x:u32)->u32{{x>>4}}",
        "^1u32".repeat(63)
    );
    let plain = chain + "fn f(x:u32)->(u32,u32){let a=h(x);let b=k(x^1u32);(a,b)}";
    let dead = plain.replacen("let a=h(x);", "let dead=7u32;let a=h(x);", 1);
    let reversed = dead.replacen(
        "let a=h(x);let b=k(x^1u32);",
        "let b=k(x^1u32);let a=h(x);",
        1,
    );
    assert_eq!(
        expression_depth(&plain, "h"),
        65,
        "the chain exceeds the summary depth cap"
    );
    let with_dead = form(&dead);
    assert_ne!(
        with_dead,
        form(&reversed),
        "the exhausted summary keeps source order"
    );
    assert_ne!(
        with_dead,
        form(&plain),
        "summary exhaustion retains the dead binding"
    );
    let inner = "{ ".repeat(64) + "1u32" + &"}".repeat(64);
    let plain = format!(
        "fn h()->u32{{{inner}}} fn k()->u32{{2u32}} fn f()->(u32,u32){{let a=h();let b=k();(a,b)}}"
    );
    let dead = plain.replacen("let a=h();", "let dead=7u32;let a=h();", 1);
    let reversed = dead.replacen("let a=h();let b=k();", "let b=k();let a=h();", 1);
    let with_dead = form(&dead);
    assert_ne!(
        with_dead,
        form(&reversed),
        "the exhausted nest keeps source order"
    );
    assert_ne!(
        with_dead,
        form(&plain),
        "lexical exhaustion retains the dead binding"
    );
}

/// One final opaque by-value callback over an array of producer values.
const FOLD_ITEMS: &str = "fn fold<const N: usize>(f: fn(u32) -> u32, v: [u32; N]) -> u32 { v.iter().map(|x| f(*x)).fold(0u32, |a, b| a ^ b) }\nfn identity(v: u32) -> u32 { v }\nfn low(x: u32) -> u32 { x & 15u32 }\n";

/// Non-twin producers with indistinguishable ordered neighborhoods, plus one helper call.
fn label_region(count: usize, shuffled: bool, extra: bool) -> String {
    let mut order: Vec<usize> = (0..count).collect();
    if shuffled {
        order.reverse();
    }
    let mut body = String::new();
    for &index in &order {
        write!(body, "let m{index} = input >> 4;").unwrap();
    }
    for index in 0..count {
        write!(body, "let n{index} = m{index} ^ 1u32;").unwrap();
    }
    if extra {
        body.push_str("let d = 7u32; ");
    }
    body.push_str("let hc = low(input);");
    let names: Vec<String> = (0..count)
        .map(|index| format!("m{index}"))
        .chain((0..count).map(|index| format!("n{index}")))
        .chain(core::iter::once("hc".to_owned()))
        .collect();
    format!(
        "{FOLD_ITEMS}fn f(input: u32) -> (u32, u32) {{ {body}fold(identity, [{}]); (hc, input) }}",
        names.join(", ")
    )
}

#[test]
fn labeling_state_exhaustion_retains_the_helper_region() {
    let within = (label_region(5, false, false), label_region(5, true, false));
    assert_eq!(
        form(&within.0),
        form(&within.1),
        "within the state budget the helper region schedules"
    );
    let over = (label_region(8, false, true), label_region(8, true, true));
    assert_ne!(
        form(&over.0),
        form(&over.1),
        "past the state budget the helper region keeps source order"
    );
}

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

/// A balanced conjunction over `leaves` copies of `t`, one of them negated.
fn final_syntax(leaves: usize, flag_first: bool) -> String {
    let tree = xor_tree("t", leaves)
        .replace('^', "&&")
        .replacen('t', "!t", 1);
    let statements = if flag_first {
        format!("let a=flag(true);let t=p;let r=!({tree});(a,r)")
    } else {
        format!("let t=p;let r=!({tree});let a=flag(true);(a,r)")
    };
    format!("fn flag(p:bool)->bool{{p}} fn f(p:bool)->(bool,bool){{{statements}}}")
}

#[derive(Default)]
struct ChainNodes {
    exprs: usize,
    leaves: usize,
}

impl syn::visit::Visit<'_> for ChainNodes {
    fn visit_expr(&mut self, expr: &syn::Expr) {
        self.exprs += 1;
        if matches!(expr, syn::Expr::Path(_)) {
            self.leaves += 1;
        }
        syn::visit::visit_expr(self, expr);
    }
}

fn chain_nodes(source: &str) -> (usize, usize) {
    let file: syn::File = syn::parse_str(source).unwrap();
    let function = file
        .items
        .iter()
        .find_map(|item| match item {
            syn::Item::Fn(function) if function.sig.ident == "f" => Some(function),
            _ => None,
        })
        .unwrap();
    let mut count = ChainNodes::default();
    for stmt in &function.block.stmts {
        if let syn::Stmt::Local(local) = stmt
            && let syn::Pat::Ident(ident) = &local.pat
            && ident.ident == "r"
        {
            let init = local.init.as_ref().unwrap();
            syn::visit::visit_expr(&mut count, &init.expr);
        }
    }
    (count.exprs, count.leaves)
}

#[test]
fn final_syntax_exhaustion_retains_the_helper_region() {
    let (exprs, leaves) = chain_nodes(&final_syntax(5460, true));
    assert_eq!(
        exprs,
        3 * leaves,
        "the source tree counts its parentheses and negations"
    );
    assert_eq!(
        3 * leaves + 7,
        16_387,
        "the emitted final syntax exceeds the syntax budget"
    );
    let over = (final_syntax(5460, true), final_syntax(5460, false));
    assert_ne!(
        form(&over.0),
        form(&over.1),
        "the exhausted final syntax retains the whole helper region"
    );
    let (exprs, leaves) = chain_nodes(&final_syntax(5459, true));
    assert_eq!(
        exprs,
        3 * leaves,
        "the source tree counts its parentheses and negations"
    );
    assert_eq!(
        3 * leaves + 7,
        16_384,
        "the emitted final syntax reaches the syntax budget"
    );
    let within = (final_syntax(5459, true), final_syntax(5459, false));
    assert_eq!(
        form(&within.0),
        form(&within.1),
        "the within-syntax budget region schedules"
    );
}

/// A caller region of `literals` producer statements plus one helper call.
fn source_cap(literals: usize, wide_last: bool, wide_tail: bool, shuffled: bool) -> String {
    let mut order: Vec<usize> = (0..literals).collect();
    if shuffled {
        order.reverse();
    }
    let mut body = String::new();
    for &index in &order {
        let wide = index + 1 == literals && wide_last;
        if wide {
            write!(body, "let v{index} = input&{index}u32|2u32;").unwrap();
        } else {
            write!(body, "let v{index} = input&{index}u32;").unwrap();
        }
    }
    body.push_str("let hc = low(input);");
    let names: Vec<String> = (0..literals)
        .map(|index| format!("v{index}"))
        .chain(core::iter::once("hc".to_owned()))
        .collect();
    write!(body, "fold(identity, [{}]);", names.join(", ")).unwrap();
    body.push_str(if wide_tail { "input|0u32" } else { "input" });
    format!(
        "fn fold<const N: usize>(f: fn(u32) -> u32, v: [u32; N]) -> u32 {{ v.iter().map(|x| f(*x)).fold(0u32, |a, b| a ^ b) }}\nfn identity(v: u32) -> u32 {{ v }}\nfn low(x: u32) -> u32 {{ x & 15u32 }}\nfn f(input: u32) -> u32 {{ {body} }}"
    )
}

fn region_nodes(source: &str) -> usize {
    let file: syn::File = syn::parse_str(source).unwrap();
    for item in &file.items {
        if let syn::Item::Fn(function) = item
            && function.sig.ident == "f"
        {
            let mut count = DeclarationNodes::default();
            syn::visit::visit_block(&mut count, &function.block);
            return count.stmt + count.expr + count.item;
        }
    }
    unreachable!("every fixture declares the function f")
}

#[test]
fn the_caller_source_cap_counts_helper_call_nodes() {
    let at_cap = (
        source_cap(3274, true, false, false),
        source_cap(3274, true, false, true),
    );
    assert_eq!(
        region_nodes(&at_cap.0),
        16_384,
        "the caller region with its helper call sits at the cap"
    );
    assert_eq!(
        form(&at_cap.0),
        form(&at_cap.1),
        "the at-cap caller region schedules"
    );
    let over = (
        source_cap(3274, true, true, false),
        source_cap(3274, true, true, true),
    );
    assert_eq!(
        region_nodes(&over.0),
        16_386,
        "the caller region passes the cap"
    );
    assert_ne!(
        form(&over.0),
        form(&over.1),
        "the over-cap caller region keeps source order"
    );
}

#[test]
fn closure_labels_include_their_transitive_helper_budget() {
    let helpers = format!(
        "fn big(p:bool)->bool{{{}p}}fn h(p:bool)->bool{{big(p);p}}fn k(p:bool)->bool{{p}}",
        "true;".repeat(8191)
    );
    let first = format!(
        "{helpers}fn f(p:bool)->(bool,bool){{let c=|q:bool|{{h(q);}};let a=k(!p);let b=k(p);(a,b)}}"
    );
    let same = format!(
        "{helpers}fn f(p:bool)->(bool,bool){{let c=|q:bool|{{h(q);}};let b=k(p);let a=k(!p);(a,b)}}"
    );
    assert_ne!(form(&first), form(&same));
}

#[test]
fn closure_inputs_mask_farther_helper_budget_roots() {
    let helpers = format!(
        "fn big(p:bool)->bool{{{}p}}fn h(p:bool)->bool{{big(p);p}}fn k(p:bool)->bool{{p}}",
        "true;".repeat(8191)
    );
    let first = format!(
        "{helpers}fn f(p:bool)->(bool,bool){{let c=|h:fn(bool)->bool|{{h(p);}};let a=k(!p);let b=k(p);(a,b)}}"
    );
    let same = format!(
        "{helpers}fn f(p:bool)->(bool,bool){{let c=|h:fn(bool)->bool|{{h(p);}};let b=k(p);let a=k(!p);(a,b)}}"
    );
    assert_eq!(form(&first), form(&same));
}

#[test]
fn original_helper_proof_work_retains_every_checked_branch() {
    let build = |count: usize, reversed: bool, dead: bool| {
        let helpers = format!(
            "fn h(p:bool)->bool{{if true{{p}}else{{{}}}}}fn k(p:bool)->bool{{p}}",
            xor_tree("p", count)
        );
        let binding = if dead { "let unused=false;" } else { "" };
        let statements = if reversed {
            "let b=k(!p);let a=h(p);"
        } else {
            "let a=h(p);let b=k(!p);"
        };
        format!("{helpers}fn f(p:bool)->(bool,bool){{{binding}{statements}(a,b)}}")
    };
    let over = form(&build(5462, false, false));
    assert_ne!(over, form(&build(5462, true, false)));
    assert_eq!(
        declaration_nodes(&build(5458, false, false), &["h", "k"]),
        16_383
    );
    assert_eq!(
        declaration_nodes(&build(5459, false, false), &["h", "k"]),
        16_386
    );
    assert_ne!(form(&build(5462, false, true)), over);
    assert_eq!(
        form(&build(5458, false, false)),
        form(&build(5458, true, false))
    );
}

#[test]
fn uncalled_nested_declarations_do_not_enter_the_summary_union() {
    let helpers = format!(
        "fn h(p:bool)->bool{{fn unused(p:bool)->bool{{{}p}}p}}fn k(p:bool)->bool{{p}}",
        "true;".repeat(8192)
    );
    let a = format!("{helpers}fn f(p:bool)->(bool,bool){{let a=h(p);let b=k(!p);(a,b)}}");
    let b = format!("{helpers}fn f(p:bool)->(bool,bool){{let b=k(!p);let a=h(p);(a,b)}}");
    assert_eq!(form(&a), form(&b));
}

#[test]
fn uncalled_nested_observers_still_mask_the_enclosing_helper() {
    for body in [
        "#[observe]p",
        "struct S;impl S{#[observe]fn m()->bool{true}}p",
        "trait T{#[observe]fn m()->bool{true}}p",
        "struct S;impl S{fn m(#[observe]x:bool)->bool{x}}p",
        "trait T{fn m(#[observe]x:bool)->bool{x}}p",
    ] {
        let helpers = format!(
            "fn h(p:bool)->bool{{fn unused(p:bool)->bool{{{body}}}p}}fn k(p:bool)->bool{{p}}"
        );
        let a = format!("{helpers}fn f(p:bool)->(bool,bool){{let a=h(p);let b=k(!p);(a,b)}}");
        let b = format!("{helpers}fn f(p:bool)->(bool,bool){{let b=k(!p);let a=h(p);(a,b)}}");
        assert_ne!(form(&a), form(&b), "{body}");
    }
}

#[test]
fn unindexed_nested_observers_exhaust_lexical_helper_ancestors() {
    for (open, close) in [("(", ")"), ("{", "}")] {
        let hidden = format!("{}#[observe]p{}", open.repeat(65), close.repeat(65));
        let helpers = format!(
            "fn h(p:bool)->bool{{fn unused(p:bool)->bool{{{hidden}}}p}}fn k(p:bool)->bool{{p}}"
        );
        let first = format!(
            "{helpers}fn f(p:bool)->(bool,bool){{let unused=false;let a=h(p);let b=k(!p);(a,b)}}"
        );
        let same = format!(
            "{helpers}fn f(p:bool)->(bool,bool){{let unused=false;let b=k(!p);let a=h(p);(a,b)}}"
        );
        let erased = first.replace("let unused=false;", "");
        assert_ne!(form(&first), form(&same));
        assert_ne!(form(&first), form(&erased));
    }
}

#[test]
fn producer_exhaustion_retains_bindings_when_value_erasure_is_disabled() {
    let prefix = "mod names{}use names::*;";
    let mut bindings = String::new();
    for index in 0..16_383 {
        write!(bindings, "let v{index}=true;").unwrap();
    }
    let first = format!("{prefix}fn f()->(bool,bool){{let a=true;let b=false;{bindings}(a,b)}}");
    let same = format!("{prefix}fn f()->(bool,bool){{let b=false;let a=true;{bindings}(a,b)}}");
    assert_ne!(form(&first), form(&same));
    assert_ne!(form(&first), form(&first.replace("let v16382=true;", "")));
}
