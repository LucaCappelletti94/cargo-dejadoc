//! Expression-shape value erasure, conditional initializers, group wrappers and expansion bounds.

use std::fmt::Write as _;

#[path = "support/grouped_file.rs"]
mod grouped;

use grouped::grouped_file;

fn form(source: &str) -> syn_canon::CanonicalForm {
    syn_canon::canonicalize(syn::parse_str(source).unwrap())
}

fn separate(left: &str, right: &str) {
    assert_ne!(form(left), form(right), "{left}\n{right}");
    assert_ne!(
        syn_canon::canonicalize_failing(syn::parse_str(left).unwrap()),
        syn_canon::canonicalize_failing(syn::parse_str(right).unwrap())
    );
}

fn grouped(source: &str) -> syn_canon::CanonicalForm {
    syn_canon::canonicalize(grouped_file(source))
}

#[test]
fn substituted_condition_initializer_folds_to_the_inline_form() {
    let original = "fn f(x:bool)->bool{let cond=x;let c=if cond&&x{true}else{false};c}";
    let inline = "fn f(x:bool)->bool{if x&&x{true}else{false}}";
    assert_eq!(form(original), form(inline));
    assert_ne!(
        syn_canon::canonicalize_failing(syn::parse_str(original).unwrap()),
        syn_canon::canonicalize_failing(syn::parse_str(inline).unwrap())
    );
    assert_eq!(form(original).body_units(), 3);
}

#[test]
fn substituted_condition_tail_folds_to_the_inline_form() {
    let original = "fn f(x:bool)->bool{let g=x;if g&&x{true}else{false}}";
    let inline = "fn f(x:bool)->bool{if x&&x{true}else{false}}";
    assert_eq!(form(original), form(inline));
    assert_ne!(
        syn_canon::canonicalize_failing(syn::parse_str(original).unwrap()),
        syn_canon::canonicalize_failing(syn::parse_str(inline).unwrap())
    );
    assert_eq!(form(original).body_units(), 3);
}

#[test]
fn cached_expansion_across_aliases_substitutes_to_the_source() {
    let original = "fn f(x:u32)->u32{let saved=x&15u32;let copied=saved;copied}";
    let inline = "fn f(x:u32)->u32{x&15u32}";
    assert_eq!(form(original), form(inline));
    assert_ne!(
        syn_canon::canonicalize_failing(syn::parse_str(original).unwrap()),
        syn_canon::canonicalize_failing(syn::parse_str(inline).unwrap())
    );
    assert_eq!(form(original).body_units(), 3);
}

#[test]
fn capture_failure_in_initializer_retains_the_complete_block() {
    let original =
        "fn f(x:u32,read:fn()->u32)->u32{let saved=x&15u32;let x=read();let copied=saved;copied}";
    let collapsed = "fn f(x:u32,read:fn()->u32)->u32{x&15u32}";
    let captured = "fn f(x:u32,read:fn()->u32)->u32{let x=read();x&15u32}";
    separate(original, collapsed);
    separate(original, captured);
    assert_eq!(form(original).body_units(), 18);
}

#[test]
fn capture_failure_in_conditional_condition_retains_the_complete_block() {
    let original =
        "fn f(x:bool,read:fn()->bool)->bool{let saved=x;let x=read();if saved{true}else{false}}";
    let collapsed = "fn f(x:bool,read:fn()->bool)->bool{x}";
    let captured = "fn f(x:bool,read:fn()->bool)->bool{let x=read();if x{true}else{false}}";
    separate(original, collapsed);
    separate(original, captured);
    assert_eq!(form(original).body_units(), 15);
}

fn alias_chain(definitions: usize) -> String {
    let mut source = String::from("fn f(x:u32)->u32{let t=x&15u32;");
    for index in 0..definitions - 1 {
        if index == 0 {
            source.push_str("let v0=t;");
        } else {
            write!(source, "let v{index}=v{};", index - 1).unwrap();
        }
    }
    if definitions == 1 {
        source.push_str("t}");
    } else {
        write!(source, "v{}}}", definitions - 2).unwrap();
    }
    source
}

#[test]
fn cached_expansion_within_the_cap_collapses_the_alias_chain() {
    let original = alias_chain(5461);
    let inline = "fn f(x:u32)->u32{x&15u32}";
    assert_eq!(form(&original), form(inline));
    assert_ne!(
        syn_canon::canonicalize_failing(syn::parse_str(&original).unwrap()),
        syn_canon::canonicalize_failing(syn::parse_str(inline).unwrap())
    );
    assert_eq!(form(&original).body_units(), 3);
}

#[test]
fn cached_expansion_over_the_cap_retains_every_definition() {
    let original = alias_chain(5462);
    let inline = "fn f(x:u32)->u32{x&15u32}";
    separate(&original, inline);
    assert_eq!(form(&original).body_units(), 27313);
}

#[test]
fn grouped_initializer_reaches_admission_and_erases() {
    let original = "fn f(x:u32)->u32{let t=x&15u32;t}";
    let inline = "fn f(x:u32)->u32{x&15u32}";
    assert_eq!(grouped(original), form(inline));
    assert_ne!(
        syn_canon::canonicalize_failing(grouped_file(original)),
        syn_canon::canonicalize_failing(syn::parse_str(inline).unwrap())
    );
    assert_eq!(grouped(original).body_units(), 3);
}

#[test]
fn grouped_call_initializer_stays_retained() {
    let original = "fn f(x:u32,read:fn()->u32)->u32{let t=read();t}";
    let collapsed = "fn f(x:u32,read:fn()->u32)->u32{read()}";
    assert_ne!(grouped(original), form(collapsed));
    assert_ne!(
        syn_canon::canonicalize_failing(grouped_file(original)),
        syn_canon::canonicalize_failing(syn::parse_str(collapsed).unwrap())
    );
}

#[test]
fn typed_operands_establish_unsuffixed_temporary_types() {
    let original = "fn f(x:u32)->u32{let t=x&15;t}";
    let inline = "fn f(x:u32)->u32{x&15}";
    assert_eq!(form(original), form(inline));
    assert_ne!(
        syn_canon::canonicalize_failing(syn::parse_str(original).unwrap()),
        syn_canon::canonicalize_failing(syn::parse_str(inline).unwrap())
    );
    assert_eq!(form(original).body_units(), 3);
}

#[test]
fn annotated_inference_keeps_the_annotation() {
    let original = "fn f(x:u32)->u32{let t:u32=x&15;t}";
    let unannotated = "fn f(x:u32)->u32{let t=x&15;t}";
    let inline = "fn f(x:u32)->u32{x&15u32}";
    separate(original, unannotated);
    separate(original, inline);
    assert_eq!(form(original).body_units(), 10);
}

#[test]
fn zero_initializer_keeps_the_declaration() {
    let original = "fn f(x:u32)->u32{let t:u32;x&15u32}";
    let inline = "fn f(x:u32)->u32{x&15u32}";
    separate(original, inline);
    assert_eq!(form(original).body_units(), 8);
}

#[test]
fn let_else_keeps_the_declaration_and_its_source() {
    let original = "fn f(x:u32)->u32{let t=Some(x&15u32);let Some(v)=t else{return 0u32};v}";
    let inline = "fn f(x:u32)->u32{x&15u32}";
    separate(original, inline);
    assert_eq!(form(original).body_units(), 18);
}

fn vec_source(literals: usize, unused: &str) -> String {
    let items = (0..literals).map(|_| "0u32").collect::<Vec<_>>().join(",");
    format!("fn f(input:u32)->u32{{let unused={unused};let values=vec![{items}];input}}")
}

#[test]
fn macro_label_over_the_cap_retains_the_complete_source() {
    let left = vec_source(16384, "7u32");
    let right = vec_source(16384, "9u32");
    separate(&left, &right);
    assert_eq!(form(&left).body_units(), 16396);
}

#[test]
fn macro_label_within_the_cap_erases_the_dead_binding() {
    let original = "fn f(input:u32)->u32{let unused=7u32;let values=vec![0u32,0u32];input}";
    let inline = "fn f(input:u32)->u32{let values=vec![0u32,0u32];input}";
    assert_eq!(form(original), form(inline));
    assert_ne!(
        syn_canon::canonicalize_failing(syn::parse_str(original).unwrap()),
        syn_canon::canonicalize_failing(syn::parse_str(inline).unwrap())
    );
    assert_eq!(form(original).body_units(), 9);
}
