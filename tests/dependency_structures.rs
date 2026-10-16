//! Structural reordering preserves labels, effects and alternative multiplicity.

use std::fmt::Write as _;

fn form(source: &str) -> syn_canon::CanonicalForm {
    syn_canon::canonicalize(syn::parse_str(source).unwrap())
}

fn failing(source: &str) -> syn_canon::CanonicalForm {
    syn_canon::canonicalize_failing(syn::parse_str(source).unwrap())
}

#[test]
fn structure_field_order_shorthand_and_alias() {
    let alias_first = "struct S{x:u32,y:u32} fn f(input:u32)->(u32,u32){let t=input;let s=S{x:t,y:input};(s.x,s.y)}";
    let alias_second = "struct S{x:u32,y:u32} fn f(input:u32)->(u32,u32){let t=input;let s=S{y:input,x:t};(s.x,s.y)}";
    assert_eq!(form(alias_first), form(alias_second));
    assert_ne!(failing(alias_first), failing(alias_second));
    let shorthand_first = "struct S{x:u32,y:u32} fn f(input:u32)->(u32,u32){let x=input;let s=S{x,y:input};(s.x,s.y)}";
    let shorthand_second = "struct S{x:u32,y:u32} fn f(input:u32)->(u32,u32){let x=input;let s=S{y:input,x};(s.x,s.y)}";
    assert_eq!(form(shorthand_first), form(shorthand_second));
    assert_ne!(failing(shorthand_first), failing(shorthand_second));
}

#[test]
fn structure_field_order_effectful_initializer_separate() {
    let first =
        "struct S{x:u32,y:u32} fn f(input:u32)->(u32,u32){let s=S{x:f(input),y:input};(s.x,s.y)}";
    let second =
        "struct S{x:u32,y:u32} fn f(input:u32)->(u32,u32){let s=S{y:input,x:f(input)};(s.x,s.y)}";
    assert_ne!(form(first), form(second));
}

#[test]
fn structure_field_order_macro_initializer_separate() {
    let first =
        "struct S{x:u32,y:u32} fn f()->(u32,u32){let s=S{x:panic!(\"x\"),y:1u32};(s.x,s.y)}";
    let second =
        "struct S{x:u32,y:u32} fn f()->(u32,u32){let s=S{y:1u32,x:panic!(\"x\")};(s.x,s.y)}";
    assert_ne!(form(first), form(second));
}

#[test]
fn structure_field_order_update_base() {
    let first = "struct S{x:u32,y:u32,z:u32} fn f(input:u32)->S{let b=S{x:0u32,y:0u32,z:0u32};S{x:input,y:input^1u32, ..b}}";
    let second = "struct S{x:u32,y:u32,z:u32} fn f(input:u32)->S{let b=S{x:0u32,y:0u32,z:0u32};S{y:input^1u32,x:input, ..b}}";
    assert_eq!(form(first), form(second));
    assert_ne!(failing(first), failing(second));
}

#[test]
fn structure_field_order_primitive_shadow() {
    let first = "struct S{x:u32,y:u32} fn f(u32:u32)->u32{let s=S{x:u32,y:1u32};s.x+s.y}";
    let second = "struct S{x:u32,y:u32} fn f(u32:u32)->u32{let s=S{y:1u32,x:u32};s.x+s.y}";
    assert_eq!(form(first), form(second));
    assert_ne!(failing(first), failing(second));
}

#[test]
fn structure_field_order_type_shadow() {
    let first = "struct S{x:u32,y:u32} fn f(input:u32)->(u32,u32){type u32=u64;let s=S{x:input,y:input&1u32};(s.x,s.y)}";
    let second = "struct S{x:u32,y:u32} fn f(input:u32)->(u32,u32){type u32=u64;let s=S{y:input&1u32,x:input};(s.x,s.y)}";
    assert_eq!(form(first), form(second));
    assert_ne!(failing(first), failing(second));
}

#[test]
fn structure_field_order_raw_member() {
    let first = "struct S{r#type:u32,name:u32} fn f(input:u32)->(u32,u32){let s=S{r#type:input,name:input};(s.r#type,s.name)}";
    let second = "struct S{r#type:u32,name:u32} fn f(input:u32)->(u32,u32){let s=S{name:input,r#type:input};(s.r#type,s.name)}";
    assert_eq!(form(first), form(second));
    assert_ne!(failing(first), failing(second));
}

#[test]
fn structure_field_order_positional_members_separate() {
    let first = "struct S(u32,u32); fn f()->(u32,u32){let s=S{0:1u32,1:2u32};(s.0,s.1)}";
    let second = "struct S(u32,u32); fn f()->(u32,u32){let s=S{1:2u32,0:1u32};(s.0,s.1)}";
    assert_ne!(form(first), form(second));
}

#[test]
fn structure_field_order_duplicate_labels_separate() {
    let first = "struct S{x:u32,y:u32} fn f()->u32{let s=S{x:1u32,x:2u32};s.x+s.x}";
    let second = "struct S{x:u32,y:u32} fn f()->u32{let s=S{x:2u32,x:1u32};s.x+s.x}";
    assert_ne!(form(first), form(second));
}

#[test]
fn structure_field_order_attributed_field_separate() {
    let first = "struct S{x:u32,y:u32} fn f()->(u32,u32){let s=S{#[a] y:2u32,x:1u32};(s.x,s.y)}";
    let second = "struct S{x:u32,y:u32} fn f()->(u32,u32){let s=S{#[a] x:1u32,y:2u32};(s.x,s.y)}";
    assert_ne!(form(first), form(second));
}

#[test]
fn structure_field_order_item_initializer() {
    let first = "struct S{x:u32,y:u32} fn f()->S{const X:S=S{y:2u32,x:1u32};X}";
    let second = "struct S{x:u32,y:u32} fn f()->S{const X:S=S{x:1u32,y:2u32};X}";
    assert_eq!(form(first), form(second));
    assert_ne!(failing(first), failing(second));
}

#[test]
fn structure_field_order_helper_total_and_effectful() {
    let total_first = "struct S{x:u32,y:u32} fn h(v:u32)->u32{v&3u32} fn f(input:u32)->(u32,u32){let s=S{x:h(input),y:h(input^1u32)};(s.x,s.y)}";
    let total_second = "struct S{x:u32,y:u32} fn h(v:u32)->u32{v&3u32} fn f(input:u32)->(u32,u32){let s=S{y:h(input^1u32),x:h(input)};(s.x,s.y)}";
    assert_eq!(form(total_first), form(total_second));
    assert_ne!(failing(total_first), failing(total_second));
    let effectful_first = "struct S{x:u32,y:u32} fn g(v:u32)->u32{panic!(\"n\")} fn f(input:u32)->(u32,u32){let s=S{x:g(input),y:input};(s.x,s.y)}";
    let effectful_second = "struct S{x:u32,y:u32} fn g(v:u32)->u32{panic!(\"n\")} fn f(input:u32)->(u32,u32){let s=S{y:input,x:g(input)};(s.x,s.y)}";
    assert_ne!(form(effectful_first), form(effectful_second));
}

#[test]
fn structure_field_order_producer_field_composition() {
    let first = "struct S{x:u32,y:u32} fn f(input:u32)->(u32,u32){let a=input&1u32;let b=input^1u32;let s=S{x:b,y:a};(s.x,s.y)}";
    let second = "struct S{x:u32,y:u32} fn f(input:u32)->(u32,u32){let b=input^1u32;let a=input&1u32;let s=S{y:a,x:b};(s.x,s.y)}";
    assert_eq!(form(first), form(second));
    assert_ne!(failing(first), failing(second));
}

#[test]
fn structure_field_order_update_base_identity() {
    let first = "struct S{x:u32,y:u32,z:u32} fn f(b:S,c:S)->S{let s=S{x:1u32,y:2u32, ..b};s}";
    let second = "struct S{x:u32,y:u32,z:u32} fn f(b:S,c:S)->S{let s=S{x:1u32,y:2u32, ..c};s}";
    assert_ne!(form(first), form(second));
}

fn big_struct_source(fields: usize, reversed: bool, item: bool) -> String {
    let mut source = String::with_capacity(fields * 24 + 96);
    source.push_str("struct S{");
    for index in 0..fields {
        write!(&mut source, "f{index}:u32,").unwrap();
    }
    source.push_str(if item {
        "} fn f()->S{const X:S=S{"
    } else {
        "} fn f()->(u32,u32){let s=S{"
    });
    let mut append = |index| write!(&mut source, "f{index}:1u32,").unwrap();
    if reversed {
        (0..fields).rev().for_each(&mut append);
    } else {
        (0..fields).for_each(&mut append);
    }
    source.push_str(if item { "};X}" } else { "};(s.f0,s.f1)}" });
    source
}

#[test]
fn structure_field_order_resource_boundary() {
    let inside = 5460;
    assert_eq!(
        form(&big_struct_source(inside, false, false)),
        form(&big_struct_source(inside, true, false))
    );
    let outside = 5461;
    assert_ne!(
        form(&big_struct_source(outside, false, false)),
        form(&big_struct_source(outside, true, false))
    );
}

#[test]
fn structure_field_order_fallback_resource_boundary() {
    let inside = 5460;
    assert_eq!(
        form(&big_struct_source(inside, false, true)),
        form(&big_struct_source(inside, true, true))
    );
    let outside = 5461;
    assert_ne!(
        form(&big_struct_source(outside, false, true)),
        form(&big_struct_source(outside, true, true))
    );
}

fn deep_block_source(depth: usize, x_first: bool) -> String {
    let fields = if x_first {
        "x:1u32,y:1u32"
    } else {
        "y:1u32,x:1u32"
    };
    let open = "{ ".repeat(depth);
    let close = " }".repeat(depth);
    format!("struct S{{x:u32,y:u32}} fn f()->u32{{{open}let s=S{{{fields}}};s.x{close}}}")
}

#[test]
fn structure_field_order_depth_endpoint() {
    let first = deep_block_source(70, true);
    let second = deep_block_source(70, false);
    assert_ne!(form(&first), form(&second));
}

#[test]
fn structure_literal_alternatives_styles() {
    let hex_first = "fn f()->bool{match 0u32{0x10|2u32=>true,_=>false}}";
    let hex_second = "fn f()->bool{match 0u32{2u32|0x10=>true,_=>false}}";
    assert_eq!(form(hex_first), form(hex_second));
    let hex_other = "fn f()->bool{match 0u32{0x11|2u32=>true,_=>false}}";
    assert_ne!(form(hex_first), form(hex_other));
    let suffix_first = "fn f()->bool{match 0u32{1u32|1=>true,_=>false}}";
    let suffix_second = "fn f()->bool{match 0u32{1|1u32=>true,_=>false}}";
    assert_eq!(form(suffix_first), form(suffix_second));
    let suffix_distinct = "fn f()->bool{match 0u32{1|1=>true,_=>false}}";
    assert_ne!(form(suffix_first), form(suffix_distinct));
    let width_first = "fn f()->u8{match 0u8{1u8|2u8=>1u8,_=>2u8}}";
    let width_second = "fn f()->u8{match 0u8{2u8|1u8=>1u8,_=>2u8}}";
    assert_eq!(form(width_first), form(width_second));
    let width_wide_first = "fn f()->u16{match 0u16{1u16|2u16=>1u16,_=>2u16}}";
    let width_wide_second = "fn f()->u16{match 0u16{2u16|1u16=>1u16,_=>2u16}}";
    assert_eq!(form(width_wide_first), form(width_wide_second));
    assert_ne!(form(width_first), form(width_wide_first));
}

#[test]
fn structure_literal_alternatives_negative() {
    let first = "fn f()->bool{match 0i32{-1|1=>true,_=>false}}";
    let second = "fn f()->bool{match 0i32{1|-1=>true,_=>false}}";
    assert_eq!(form(first), form(second));
    let positive = "fn f()->bool{match 0i32{1|1=>true,_=>false}}";
    assert_ne!(form(first), form(positive));
}

#[test]
fn structure_literal_alternatives_kinds_and_multiplicity() {
    let multiplicity_first = "fn f()->bool{match 0u32{2|1|1=>true,_=>false}}";
    let multiplicity_second = "fn f()->bool{match 0u32{1|2|1=>true,_=>false}}";
    let multiplicity_third = "fn f()->bool{match 0u32{1|1|2=>true,_=>false}}";
    assert_eq!(form(multiplicity_first), form(multiplicity_second));
    assert_eq!(form(multiplicity_first), form(multiplicity_third));
    let char_first = "fn f()->bool{match 'a'{'a'|'b'=>true,_=>false}}";
    let char_second = "fn f()->bool{match 'a'{'b'|'a'=>true,_=>false}}";
    assert_eq!(form(char_first), form(char_second));
    let bool_first = "fn f()->bool{match true{true|false=>true,_=>false}}";
    let bool_second = "fn f()->bool{match true{false|true=>true,_=>false}}";
    assert_eq!(form(bool_first), form(bool_second));
}

#[test]
fn structure_literal_alternatives_unchanged_arms() {
    let guarded_first = "fn f(input:u32)->bool{match input{2|1 if input==2u32=>true,_=>false}}";
    let guarded_second = "fn f(input:u32)->bool{match input{1|2 if input==2u32=>true,_=>false}}";
    assert_ne!(form(guarded_first), form(guarded_second));
    let binding_first = "fn f(input:u32)->bool{match input{1|x=>true,_=>false}}";
    let binding_second = "fn f(input:u32)->bool{match input{x|1=>true,_=>false}}";
    assert_ne!(form(binding_first), form(binding_second));
    let range_first = "fn f(input:u32)->bool{match input{4|1..=3=>true,_=>false}}";
    let range_second = "fn f(input:u32)->bool{match input{1..=3|4=>true,_=>false}}";
    assert_ne!(form(range_first), form(range_second));
    let path_first = "enum E{A} fn f(input:u32)->bool{match input{E::A|1=>true,_=>false}}";
    let path_second = "enum E{A} fn f(input:u32)->bool{match input{1|E::A=>true,_=>false}}";
    assert_ne!(form(path_first), form(path_second));
    let nested_first = "fn f(input:u32)->bool{match input{3|(1|2)=>true,_=>false}}";
    let nested_second = "fn f(input:u32)->bool{match input{(1|2)|3=>true,_=>false}}";
    assert_ne!(form(nested_first), form(nested_second));
    let macro_first = "fn f(input:u32)->bool{match input{m!(x)|1=>true,_=>false}}";
    let macro_second = "fn f(input:u32)->bool{match input{1|m!(x)=>true,_=>false}}";
    assert_ne!(form(macro_first), form(macro_second));
}

fn literal_arm_source(count: usize, swapped: bool) -> String {
    let mut source = String::with_capacity(count * 6 + 64);
    source.push_str("fn f()->bool{match 0u32{");
    for value in 0..count {
        if value != 0 {
            source.push('|');
        }
        let literal = match (value, swapped) {
            (0, true) => 1,
            (1, true) => 0,
            _ => value,
        };
        write!(&mut source, "{literal}").unwrap();
    }
    source.push_str("=>true,_=>false}}");
    source
}

#[test]
fn structure_literal_alternatives_resource_boundary() {
    let inside = 16384;
    assert_eq!(
        form(&literal_arm_source(inside, false)),
        form(&literal_arm_source(inside, true))
    );
    let outside = 16385;
    assert_ne!(
        form(&literal_arm_source(outside, false)),
        form(&literal_arm_source(outside, true))
    );
}

#[test]
fn structure_named_fields() {
    let first = "struct S{x:u32,y:u32} fn f(input:u32)->(u32,u32){let s=S{x:input&15u32,y:input>>4};(s.x,s.y)}";
    let second = "struct S{x:u32,y:u32} fn f(input:u32)->(u32,u32){let s=S{y:input>>4,x:input&15u32};(s.x,s.y)}";
    assert_eq!(form(first), form(second));
    assert_ne!(
        syn_canon::canonicalize_failing(syn::parse_str(first).unwrap()),
        syn_canon::canonicalize_failing(syn::parse_str(second).unwrap())
    );
}

#[test]
fn structure_literal_alternatives() {
    let first = "fn f(input:u32)->bool{match input{1|2=>true,_=>false}}";
    let second = "fn f(input:u32)->bool{match input{2|1=>true,_=>false}}";
    assert_eq!(form(first), form(second));
    assert_ne!(
        syn_canon::canonicalize_failing(syn::parse_str(first).unwrap()),
        syn_canon::canonicalize_failing(syn::parse_str(second).unwrap())
    );
}

#[test]
fn structure_unsupported_c_string_patterns() {
    let first = "fn f(input:&std::ffi::CStr)->bool{match input{c\"a\"|c\"b\"=>true,_=>false}}";
    let second = "fn f(input:&std::ffi::CStr)->bool{match input{c\"b\"|c\"a\"=>true,_=>false}}";
    assert_ne!(form(first), form(second));
    assert_ne!(failing(first), failing(second));
}

#[test]
fn structure_literal_domains() {
    for (ty, left, right, changed) in [
        ("f64", "1.0|-2.0", "-2.0|1.0", "-2.0|3.0"),
        ("u8", "b'a'|b'b'", "b'b'|b'a'", "b'b'|b'c'"),
        ("&[u8]", "b\"a\"|b\"b\"", "b\"b\"|b\"a\"", "b\"b\"|b\"c\""),
    ] {
        let source = |alternatives| {
            format!("fn f(input:{ty})->bool{{match input{{{alternatives}=>true,_=>false}}}}")
        };
        assert_eq!(form(&source(left)), form(&source(right)));
        assert_ne!(form(&source(left)), form(&source(changed)));
        assert_ne!(failing(&source(left)), failing(&source(right)));
    }
}

#[test]
fn structure_const_update_base_preserves_payload() {
    let source = |fields, value| {
        format!(
            "struct S{{x:u32,y:u32,z:u32}} fn f()->(u32,u32,u32){{const X:S=S{{{fields},..S{{x:0u32,y:0u32,z:{value}u32}}}};(X.x,X.y,X.z)}}"
        )
    };
    assert_eq!(
        form(&source("x:1u32,y:2u32", 3)),
        form(&source("y:2u32,x:1u32", 3))
    );
    assert_ne!(
        form(&source("x:1u32,y:2u32", 3)),
        form(&source("y:2u32,x:1u32", 4))
    );
}

#[test]
fn structure_attributed_update_base_retains_field_order() {
    let source = |fields| {
        format!(
            "struct S{{x:u32,y:u32,z:u32}} fn f(base:S)->S{{S{{{fields},..#[cfg(all())] base}}}}"
        )
    };
    assert_ne!(
        form(&source("x:1u32,y:2u32")),
        form(&source("y:2u32,x:1u32"))
    );
}

#[test]
fn structure_empty_constructor_preserves_neighbor_admission() {
    let source = |fields| {
        format!(
            "struct E{{}} struct S{{x:u32,y:u32}} fn f()->(E,(u32,u32)){{let e=E{{}};let s=S{{{fields}}};(e,(s.x,s.y))}}"
        )
    };
    assert_eq!(
        form(&source("x:1u32,y:2u32")),
        form(&source("y:2u32,x:1u32"))
    );
}

#[test]
fn structure_constructed_literal_attribute_retains_order() {
    let source = |alternatives| {
        format!(
            "fn f(input:u32)->bool{{match input{{#[cfg(all())] {alternatives}=>true,_=>false}}}}"
        )
    };
    let attributed = |source: &str| {
        let mut file: syn::File = syn::parse_str(source).unwrap();
        let syn::Item::Fn(function) = file.items.first_mut().unwrap() else {
            panic!("function expected")
        };
        let syn::Stmt::Expr(syn::Expr::Match(expression), _) =
            function.block.stmts.first_mut().unwrap()
        else {
            panic!("match expected")
        };
        let arm = expression.arms.first_mut().unwrap();
        let attrs = core::mem::take(&mut arm.attrs);
        let syn::Pat::Or(alternatives) = &mut arm.pat else {
            panic!("alternatives expected")
        };
        let syn::Pat::Lit(literal) = alternatives.cases.first_mut().unwrap() else {
            panic!("literal expected")
        };
        literal.attrs = attrs;
        syn_canon::canonicalize(file)
    };
    assert_ne!(attributed(&source("1|2")), attributed(&source("2|1")));
}

#[test]
fn structure_field_order_composes_with_double_negation() {
    let a = "struct S { x: u32, y: bool } fn f(input: u32, p: bool) -> (u32, bool) { let a = input & 1u32; let b = input ^ 1u32; let s = S { x: b, y: !!p }; (s.x, s.y) }";
    let b = "struct S { x: u32, y: bool } fn f(input: u32, p: bool) -> (u32, bool) { let b = input ^ 1u32; let a = input & 1u32; let s = S { y: p, x: b }; (s.x, s.y) }";
    assert_eq!(form(a), form(b));
    assert_ne!(failing(a), failing(b));
}

#[test]
fn structure_literal_alternatives_compose_with_inline_boolean_algebra() {
    let a = "fn f(input: u32, p: bool) -> (bool, bool) { let m = match input { 1 | 2 => true, _ => false }; (!!p, m) }";
    let b = "fn f(input: u32, p: bool) -> (bool, bool) { let m = match input { 2 | 1 => true, _ => false }; (p, m) }";
    assert_eq!(form(a), form(b));
    assert_ne!(failing(a), failing(b));
}

#[test]
fn structure_literal_alternatives_match_presence_keeps_surrounding_scheduling_conservative() {
    // A match statement is not a proven total primitive producer. Its
    // presence anywhere in a block closes the open scheduling region, so a
    // single-use boolean alias elsewhere in the same block stays retained
    // rather than erased. Inline algebraic folds still apply independently.
    let a = "fn f(input: u32, p: bool) -> (bool, u32) { let gate = !!p; let m = match input { 1u32 => 10u32, _ => 0u32 }; (gate, m) }";
    let b = "fn f(input: u32, p: bool) -> (bool, u32) { let m = match input { 1u32 => 10u32, _ => 0u32 }; (p, m) }";
    assert_ne!(form(a), form(b));
}
