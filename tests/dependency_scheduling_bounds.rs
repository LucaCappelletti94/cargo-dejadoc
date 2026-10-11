//! Public dependency-scheduling resource boundaries.

use std::fmt::Write as _;

fn form(source: &str) -> String {
    syn_canon::canonicalize(syn::parse_str(source).unwrap()).to_string()
}

#[derive(Default)]
struct SourceNodes {
    nodes: usize,
}

impl syn::visit::Visit<'_> for SourceNodes {
    fn visit_stmt(&mut self, stmt: &syn::Stmt) {
        self.nodes += 1;
        syn::visit::visit_stmt(self, stmt);
    }
    fn visit_expr(&mut self, expr: &syn::Expr) {
        self.nodes += 1;
        syn::visit::visit_expr(self, expr);
    }
    fn visit_item(&mut self, item: &syn::Item) {
        self.nodes += 1;
        syn::visit::visit_item(self, item);
    }
}

/// The source nodes the values budget measures inside the fixture body.
fn source_nodes(source: &str) -> usize {
    let file: syn::File = syn::parse_str(source).unwrap();
    for item in &file.items {
        if let syn::Item::Fn(fn_item) = item
            && fn_item.sig.ident == "f"
        {
            let mut count = SourceNodes::default();
            syn::visit::visit_block(&mut count, &fn_item.block);
            return count.nodes;
        }
    }
    unreachable!("every fixture declares the function f")
}

/// One final opaque by-value callback over an array of producer values.
const FOLD_ITEMS: &str = "fn fold<const N: usize>(f: fn(u32) -> u32, v: [u32; N]) -> u32 { v.iter().map(|x| f(*x)).fold(0u32, |a, b| a ^ b) }\nfn identity(v: u32) -> u32 { v }\n";

fn consume_array(names: &[String]) -> String {
    format!("fold(identity, [{}]);\n", names.join(", "))
}

/// A balanced bitwise OR over unordered producer names.
fn balanced_or(names: &[String]) -> String {
    if names.len() == 1 {
        return names[0].clone();
    }
    let mid = names.len() / 2;
    format!(
        "({}) | ({})",
        balanced_or(&names[..mid]),
        balanced_or(&names[mid..])
    )
}

/// Non-twin producers with indistinguishable ordered neighborhoods.
fn symmetry_region(count: usize, shuffled: bool, extra: bool) -> String {
    let mut order: Vec<usize> = (0..count).collect();
    if shuffled {
        order.reverse();
    }
    let mut body = String::new();
    for &i in &order {
        write!(body, "let m{i} = input >> 4;").unwrap();
    }
    for i in 0..count {
        write!(body, "let n{i} = m{i} ^ 1u32;").unwrap();
    }
    if extra {
        body.push_str("let d = 7u32; ");
    }
    let names: Vec<String> = (0..count)
        .map(|i| format!("m{i}"))
        .chain((0..count).map(|i| format!("n{i}")))
        .collect();
    format!(
        "{FOLD_ITEMS}fn f(input: u32) -> u32 {{ {body}fold(identity, [{}]); input }}",
        balanced_or(&names)
    )
}

fn chain(prefix: &str, seed: u32, marker_first: bool) -> String {
    let mut body = String::new();
    let marker = "let marker = input >> 4;";
    if marker_first {
        body.push_str(marker);
    }
    write!(body, "let {prefix}0 = input & {seed}u32;").unwrap();
    for i in 1..65 {
        write!(body, "let {prefix}{i} = {prefix}{} ^ 1u32;", i - 1).unwrap();
    }
    if !marker_first {
        body.push_str(marker);
    }
    let mut names: Vec<String> = (0..65).map(|i| format!("{prefix}{i}")).collect();
    names.push("marker".to_string());
    let consume = consume_array(&names);
    format!("{FOLD_ITEMS}fn f(input: u32) -> (u32, u32) {{ {body}{consume}({prefix}64, marker) }}")
}

#[test]
fn a_65_node_dependency_chain_labels_deterministically() {
    let a = chain("v", 15, true);
    let b = chain("w", 15, false);
    assert!(
        source_nodes(&a) <= 16_384,
        "the chain fits the source budget"
    );
    assert_eq!(form(&a), form(&b), "a 65-node chain is deterministic");
    let c = chain("v", 14, true);
    assert_ne!(form(&a), form(&c), "the chain form tracks structure");
}

#[test]
fn a_symmetric_region_within_the_state_budget_is_scheduled() {
    let a = symmetry_region(5, false, false);
    let b = symmetry_region(5, true, false);
    assert_eq!(form(&a), form(&b), "within-budget symmetry schedules");
}

#[test]
fn a_symmetric_region_over_the_state_budget_keeps_source_order() {
    let a = symmetry_region(8, false, true);
    let b = symmetry_region(8, true, true);
    let form_a = form(&a);
    assert_ne!(
        form_a,
        form(&a.replace("let d = 7u32;", "let d = 9u32;")),
        "state exhaustion retains unused declaration identity"
    );
    assert_ne!(form_a, form(&b), "over-budget region keeps source order");
}

/// Distinct producers retain their multiplicity across node limits.
fn literal_region(count: usize, shuffled: bool, consumed: bool) -> String {
    let mut order: Vec<usize> = (0..count).collect();
    if shuffled {
        order.reverse();
    }
    let mut body = String::new();
    for &i in &order {
        write!(body, "let v{i} = input & {i}u32;").unwrap();
    }
    let (header, consume) = if consumed {
        let names: Vec<String> = (0..count).map(|i| format!("v{i}")).collect();
        (FOLD_ITEMS, consume_array(&names))
    } else {
        ("", String::new())
    };
    format!("{header}fn f(input: u32) -> u32 {{ {body}{consume}input }}")
}

#[test]
fn a_region_at_the_node_cap_falls_back_to_source_order() {
    let a = literal_region(16_384, false, false);
    let b = literal_region(16_384, true, false);
    assert_eq!(
        source_nodes(&a),
        65_538,
        "the complete source AST exceeds the budget"
    );
    assert_ne!(
        form(&a),
        form(&b),
        "over-source-cap region keeps source order"
    );
}

#[test]
fn a_region_over_the_node_cap_stays_opaque() {
    let a = literal_region(16_385, false, false);
    let b = literal_region(16_385, true, false);
    assert_eq!(
        source_nodes(&a),
        65_542,
        "the complete source AST exceeds the budget"
    );
    assert_ne!(form(&a), form(&b), "over-cap region keeps source order");
}

#[test]
fn a_region_within_the_source_cap_schedules_canonically() {
    let a = literal_region(3_275, false, true);
    let b = literal_region(3_275, true, true);
    assert_eq!(
        source_nodes(&a),
        16_382,
        "the region sits at the source endpoint"
    );
    assert_eq!(form(&a), form(&b), "at-endpoint region schedules");
}

#[test]
fn a_region_over_the_source_cap_keeps_source_order() {
    let a = literal_region(3_276, false, true);
    let b = literal_region(3_276, true, true);
    assert_eq!(
        source_nodes(&a),
        16_387,
        "the region passes the source endpoint"
    );
    assert_ne!(
        form(&a),
        form(&b),
        "over-endpoint region keeps source order"
    );
}

/// A function whose two distinct producers sit inside `depth` nested
/// blocks.
fn nested_region(depth: usize, shuffled: bool) -> String {
    let (first, second) = if shuffled {
        ("let b = input >> 4;", "let a = input & 15u32;")
    } else {
        ("let a = input & 15u32;", "let b = input >> 4;")
    };
    let mut source = String::from("fn f(input: u32) -> u32 { ");
    for _ in 0..depth {
        source.push_str("{ ");
    }
    source.push_str(first);
    source.push(' ');
    source.push_str(second);
    for _ in 0..depth {
        source.push_str(" }");
    }
    source.push_str(" input }");
    source
}

#[test]
fn a_block_at_the_nesting_cap_is_still_analyzed() {
    // The producer block sits at nesting level 64, at the bound.
    let a = nested_region(63, false);
    let b = nested_region(63, true);
    assert_eq!(form(&a), form(&b), "at-cap block analyzes");
}

#[test]
fn a_block_over_the_nesting_cap_stays_opaque() {
    // The producer block sits at nesting level 65, over the bound.
    let a = nested_region(64, false);
    let b = nested_region(64, true);
    assert_ne!(form(&a), form(&b), "over-cap block keeps source order");
}

fn future_region(
    prefix: &str,
    reversed: bool,
    changed: bool,
    depth: usize,
    consumed: bool,
) -> String {
    let declarations = if reversed {
        "let second=2u32; let first=1u32;"
    } else {
        "let first=1u32; let second=2u32;"
    };
    let header = if consumed { FOLD_ITEMS } else { "" };
    let mut source = format!(
        "{header}fn f()->(u32,u32){{{declarations} println!(\"barrier\"); let {prefix}0=3u32;"
    );
    for index in 1..=depth {
        write!(
            source,
            "let {prefix}{index}={prefix}{} ^ {prefix}{};",
            index - 1,
            index - 1,
        )
        .unwrap();
    }
    let input = if changed { "second" } else { "first" };
    let consume = if consumed {
        let names: Vec<String> = (0..=depth).map(|i| format!("{prefix}{i}")).collect();
        consume_array(&names)
    } else {
        String::new()
    };
    write!(
        source,
        "{consume}let last={input}^{prefix}{depth}; (second,last)}}"
    )
    .unwrap();
    source
}

#[test]
fn shared_future_region_dependencies_preserve_schedule_and_origins() {
    let original = future_region("v", false, false, 32, true);
    let scheduled = future_region("w", true, false, 32, true);
    let changed = future_region("v", false, true, 32, true);
    assert_eq!(form(&original), form(&scheduled));
    assert_ne!(form(&original), form(&changed));
}

#[test]
fn projected_future_nodes_over_the_cap_retain_the_entire_region() {
    let source = |reversed| {
        future_region("v", reversed, false, 16_382, false)
            .replace("(second,last)", "let borrowed = &v16382; (second,last)")
    };
    assert_ne!(form(&source(false)), form(&source(true)));
}

/// A future tree connected to the preceding producer region.
fn balanced_future_region(reversed: bool, nodes: usize, consumed: bool) -> String {
    let declarations = if reversed {
        "let second=2u32; let first=1u32;"
    } else {
        "let first=1u32; let second=2u32;"
    };
    let header = if consumed { FOLD_ITEMS } else { "" };
    let mut source = format!("{header}fn f()->(u32,u32){{{declarations} println!(\"barrier\");");
    for index in (0..nodes).rev() {
        let left = 2 * index + 1;
        let right = left + 1;
        if right < nodes {
            write!(source, "let v{index}=v{left} ^ v{right};").unwrap();
        } else if left < nodes {
            write!(source, "let v{index}=v{left} ^ 1u32;").unwrap();
        } else {
            write!(source, "let v{index}={index}u32;").unwrap();
        }
    }
    let consume = if consumed {
        let names: Vec<String> = (0..nodes).map(|i| format!("v{i}")).collect();
        consume_array(&names)
    } else {
        String::new()
    };
    source.push_str(&consume);
    source.push_str("let last=first^v0; (second,last)}");
    source
}

#[test]
fn a_balanced_future_tree_over_the_source_cap_keeps_source_order() {
    let a = balanced_future_region(false, 8_200, false);
    let b = balanced_future_region(true, 8_200, false);
    assert_eq!(
        source_nodes(&a),
        24_613,
        "the complete source AST exceeds the budget"
    );
    assert_ne!(
        form(&a),
        form(&b),
        "over-source-cap tree keeps source order"
    );
}

#[test]
fn a_balanced_future_tree_within_the_source_cap_schedules_canonically() {
    let a = balanced_future_region(false, 2_047, true);
    let b = balanced_future_region(true, 2_047, true);
    assert_eq!(source_nodes(&a), 8_205, "the tree fits the source budget");
    assert_eq!(
        form(&a),
        form(&b),
        "within-source-cap tree schedules canonically"
    );
}

fn linear_future_region(reversed: bool, nodes: usize, seed: u32, consumed: bool) -> String {
    let declarations = if reversed {
        "let second=2u32; let first=1u32;"
    } else {
        "let first=1u32; let second=2u32;"
    };
    let header = if consumed { FOLD_ITEMS } else { "" };
    let mut source = format!("{header}fn f()->(u32,u32){{{declarations} println!(\"barrier\");");
    for index in (0..nodes).rev() {
        if index + 1 == nodes {
            write!(source, "let v{index}={seed}u32;").unwrap();
        } else {
            write!(source, "let v{index}=v{}^1u32;", index + 1).unwrap();
        }
    }
    let consume = if consumed {
        let names: Vec<String> = (0..nodes).map(|i| format!("v{i}")).collect();
        consume_array(&names)
    } else {
        String::new()
    };
    source.push_str(&consume);
    source.push_str("let last=first^v0; (second,last)}");
    source
}

#[test]
fn linear_future_region_over_the_source_cap_keeps_source_order() {
    let a = linear_future_region(false, 8_200, 8_199, false);
    let b = linear_future_region(true, 8_200, 8_199, false);
    assert_eq!(
        source_nodes(&a),
        32_811,
        "the complete source AST exceeds the budget"
    );
    assert_ne!(
        form(&a),
        form(&b),
        "over-source-cap chain keeps source order"
    );
}

#[test]
fn linear_future_regions_preserve_their_unique_dependency_order() {
    let source = linear_future_region(false, 3_273, 3_272, true);
    assert_eq!(
        source_nodes(&source),
        16_381,
        "the chain sits at the source endpoint"
    );
    let original = form(&source);
    assert_eq!(
        original,
        form(&linear_future_region(true, 3_273, 3_272, true))
    );
    assert_ne!(
        original,
        form(&linear_future_region(false, 3_273, 3_273, true))
    );
}
