//! Storage, version and scope boundaries of primitive value erasure.

fn separate(left: &str, right: &str) {
    assert_ne!(
        syn_canon::canonicalize(syn::parse_str(left).unwrap()),
        syn_canon::canonicalize(syn::parse_str(right).unwrap()),
        "{left}\n{right}"
    );
    assert_ne!(
        syn_canon::canonicalize_failing(syn::parse_str(left).unwrap()),
        syn_canon::canonicalize_failing(syn::parse_str(right).unwrap())
    );
}

#[test]
fn aliases_keep_original_versions_and_storage() {
    for (left, right) in [
        (
            "fn f(mut x:u32)->u32{let y=x;x=7u32;y}",
            "fn f(mut x:u32)->u32{x=7u32;x}",
        ),
        (
            "fn f(x:u32)->bool{let y=x;core::ptr::eq(&x,&y)}",
            "fn f(x:u32)->bool{core::ptr::eq(&x,&x)}",
        ),
        (
            "fn f(mut x:u32)->u32{let y=x;modify(&mut x);y}",
            "fn f(mut x:u32)->u32{modify(&mut x);x}",
        ),
        (
            "fn f(mut x:u32)->u32{let y=x;let mut c=||x=7u32;c();y}",
            "fn f(mut x:u32)->u32{let mut c=||x=7u32;c();x}",
        ),
        (
            "fn f(x:u32){let y=x;println!(\"{y}\");}",
            "fn f(x:u32){println!(\"{y}\");}",
        ),
        (
            "fn f(x:u32)->u32{let y=x;implicit!();y}",
            "fn f(x:u32)->u32{implicit!();x}",
        ),
        (
            "#[observer] fn f(x:u32)->u32{let y=x;y}",
            "#[observer] fn f(x:u32)->u32{x}",
        ),
    ] {
        separate(left, right);
    }
}

#[test]
fn temporary_evaluation_and_constraints_remain_observable() {
    for (left, right) in [
        (
            "fn f()->(u32,u32){let x=next();(x,x)}",
            "fn f()->(u32,u32){(next(),next())}",
        ),
        (
            "fn f(){let x=borrow(&Guard);point();consume(x);}",
            "fn f(){point();consume(borrow(&Guard));}",
        ),
        (
            "fn f(){let unused=if true{0}else{0u8};consume(unused);}",
            "fn f(){let unused=0;consume(unused);}",
        ),
        ("fn f(x:u8)->u8{let y:u8=x;y}", "fn f(x:u8)->u8{x}"),
        ("fn f()->u32{let unused=effect();3u32}", "fn f()->u32{3u32}"),
        ("fn f()->u8{let unused=255u8+1u8;3u8}", "fn f()->u8{3u8}"),
        (
            "struct Guard; fn f()->u8{let unused=Guard;3u8}",
            "struct Guard; fn f()->u8{3u8}",
        ),
    ] {
        separate(left, right);
    }
}

#[test]
fn tuple_positions_modes_and_simultaneous_initialization_remain_distinct() {
    for (left, right) in [
        (
            "fn f(x:u32)->(u32,u32){let(a,b)=(x&15u32,x>>4);(a,b)}",
            "fn f(x:u32)->(u32,u32){let(a,b)=(x>>4,x&15u32);(a,b)}",
        ),
        (
            "fn f(x:u32){let(ref a,b)=(x,x);consume(a,b);}",
            "fn f(x:u32){let(a,b)=(x,x);consume(a,b);}",
        ),
        (
            "fn f(a:u32)->(u32,u32){let(a,b)=(3u32,a);(a,b)}",
            "fn f(a:u32)->(u32,u32){let a=3u32;let b=a;(a,b)}",
        ),
        (
            "fn f()->(u32,u32){let(a,b)=(next(),next());(a,b)}",
            "fn f()->(u32,u32){let a=next();(a,a)}",
        ),
    ] {
        separate(left, right);
    }
}

#[test]
fn retained_shadowing_never_captures_an_expanded_source_reference() {
    let original =
        "fn f(x:u32,y:u32,read:fn(u32)->u32)->u32{let saved=x&15u32;let x=read(y);saved}";
    let captured = "fn f(x:u32,y:u32,read:fn(u32)->u32)->u32{let x=read(y);x&15u32}";
    let retained = syn_canon::canonicalize(syn::parse_str(original).unwrap());
    assert_ne!(
        retained,
        syn_canon::canonicalize(syn::parse_str(captured).unwrap())
    );
    assert_eq!(retained.body_units(), 14);
}

#[test]
fn ancestor_raw_storage_keeps_child_copies_at_their_original_read() {
    separate(
        "fn f(mut source:u32)->u32{let pointer=&raw mut source;{let saved=source;unsafe{*pointer=2u32;}saved}}",
        "fn f(mut source:u32)->u32{let pointer=&raw mut source;{unsafe{*pointer=2u32;}source}}",
    );
    separate(
        "fn f()->u32{let mut source=1u32;let pointer=&raw mut source;{let saved=source;unsafe{*pointer=2u32;}saved}}",
        "fn f()->u32{let mut source=1u32;let pointer=&raw mut source;{unsafe{*pointer=2u32;}source}}",
    );
}

#[test]
fn ancestor_storage_restrictions_keep_fresh_bindings_and_functions_independent() {
    for (left, right) in [
        (
            "fn f(mut source:u32)->u32{let pointer=&raw mut source;{let source=7u32;let saved=source;unsafe{*pointer=2u32;}saved}}",
            "fn f(mut source:u32)->u32{let pointer=&raw mut source;{unsafe{*pointer=2u32;}7u32}}",
        ),
        (
            "fn f(mut source:u32)->u32{let pointer=&raw mut source;fn child(input:u32)->u32{let saved=input;saved^3u32}child(source)}",
            "fn f(mut source:u32)->u32{let pointer=&raw mut source;fn child(input:u32)->u32{input^3u32}child(source)}",
        ),
    ] {
        assert_eq!(
            syn_canon::canonicalize(syn::parse_str(left).unwrap()),
            syn_canon::canonicalize(syn::parse_str(right).unwrap()),
        );
        assert_ne!(
            syn_canon::canonicalize_failing(syn::parse_str(left).unwrap()),
            syn_canon::canonicalize_failing(syn::parse_str(right).unwrap()),
        );
    }
}

#[test]
fn ancestor_closure_input_storage_keeps_child_copies_at_their_original_read() {
    separate(
        "fn f(source:u32)->u32{let run=|mut value:u32|{let pointer=&raw mut value;{let saved=value;unsafe{*pointer=2u32;}saved}};run(source)}",
        "fn f(source:u32)->u32{let run=|mut value:u32|{let pointer=&raw mut value;{unsafe{*pointer=2u32;}value}};run(source)}",
    );
}

#[test]
fn implicit_combining_mark_format_capture_preserves_the_value_binding() {
    separate(
        "fn f(){let cafe\u{301}=1u32;println!(\"{cafe\u{301}}\");}",
        "fn f(){println!(\"{cafe\u{301}}\");}",
    );
    separate(
        "fn f(){let x\u{301}=1u32;println!(\"{x\u{301}}\");}",
        "fn f(){println!(\"{x\u{301}}\");}",
    );
}

#[test]
fn unreachable_tail_references_keep_their_original_declarations() {
    separate(
        "fn f()->u32{let value=1u32;return 0u32;value}",
        "fn f()->u32{let value=2u32;return 0u32;value}",
    );
}

#[test]
fn unsupported_total_tail_scopes_keep_outer_value_sources() {
    separate(
        "fn f(input:u32)->u32{let value=input&15u32;{value}}",
        "fn f(input:u32)->u32{let value=input&7u32;{value}}",
    );
}
