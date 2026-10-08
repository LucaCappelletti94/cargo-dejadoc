//! Public dependency-scheduling resource boundaries.

use std::fmt::Write as _;

fn form(source: &str) -> String {
    syn_canon::canonicalize(syn::parse_str(source).unwrap()).to_string()
}

/// Non-twin producers with indistinguishable ordered neighborhoods.
fn symmetry_region(count: usize, shuffled: bool) -> String {
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
    format!("fn f(input: u32) -> u32 {{ {body} input }}")
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
    format!("fn f(input: u32) -> (u32, u32) {{ {body} ({prefix}64, marker) }}")
}

#[test]
fn a_65_node_dependency_chain_labels_deterministically() {
    let a = chain("v", 15, true);
    let b = chain("w", 15, false);
    assert_eq!(form(&a), form(&b), "a 65-node chain is deterministic");
    let c = chain("v", 14, true);
    assert_ne!(form(&a), form(&c), "the chain form tracks structure");
}

#[test]
fn a_symmetric_region_within_the_state_budget_is_scheduled() {
    let a = symmetry_region(5, false);
    let b = symmetry_region(5, true);
    assert_eq!(form(&a), form(&b), "within-budget symmetry schedules");
}

#[test]
fn a_symmetric_region_over_the_state_budget_keeps_source_order() {
    let a = symmetry_region(8, false);
    let b = symmetry_region(8, true);
    assert_ne!(form(&a), form(&b), "over-budget region keeps source order");
}

/// Distinct producers retain their multiplicity across node limits.
fn literal_region(count: usize, shuffled: bool) -> String {
    let mut order: Vec<usize> = (0..count).collect();
    if shuffled {
        order.reverse();
    }
    let mut body = String::new();
    for &i in &order {
        write!(body, "let v{i} = input & {i}u32;").unwrap();
    }
    format!("fn f(input: u32) -> u32 {{ {body} input }}")
}

#[test]
fn a_region_at_the_node_cap_is_still_labeled() {
    let a = literal_region(16_384, false);
    let b = literal_region(16_384, true);
    assert_eq!(form(&a), form(&b), "at-cap region labels");
}

#[test]
fn a_region_over_the_node_cap_stays_opaque() {
    let a = literal_region(16_385, false);
    let b = literal_region(16_385, true);
    assert_ne!(form(&a), form(&b), "over-cap region keeps source order");
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

fn future_region(prefix: &str, reversed: bool, changed: bool, depth: usize) -> String {
    let declarations = if reversed {
        "let second=2u32; let first=1u32;"
    } else {
        "let first=1u32; let second=2u32;"
    };
    let mut source =
        format!("fn f()->(u32,u32){{{declarations} println!(\"barrier\"); let {prefix}0=3u32;");
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
    write!(source, "let last={input}^{prefix}{depth}; (second,last)}}").unwrap();
    source
}

#[test]
fn shared_future_region_dependencies_preserve_schedule_and_origins() {
    let original = future_region("v", false, false, 32);
    let scheduled = future_region("w", true, false, 32);
    let changed = future_region("v", false, true, 32);
    assert_eq!(form(&original), form(&scheduled));
    assert_ne!(form(&original), form(&changed));
}

#[test]
fn projected_future_nodes_over_the_cap_retain_the_entire_region() {
    let source = |reversed| {
        future_region("v", reversed, false, 16_382)
            .replace("(second,last)", "let borrowed = &v16382; (second,last)")
    };
    assert_ne!(form(&source(false)), form(&source(true)));
}
