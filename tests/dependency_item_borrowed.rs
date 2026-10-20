//! Local helper composition in borrowed function views.

#[path = "support/function.rs"]
mod functions;
use functions::function;
#[test]
fn borrowed_views_compose_local_helpers_with_parameter_copies() {
    let source: syn::File = syn::parse_str(
        "
        fn first(input: u32, gate: bool) -> (u32, u32, bool) {
            fn one(value: u32) -> u32 { ((value & 15u32) ^ 3u32) | ((value >> 4u32) & 31u32) }
            fn two(value: u32) -> u32 { ((value | 4u32) ^ 3u32) & ((value >> 2u32) | 7u32) }
            let copied = input;
            (one(copied), two(input), if gate { true } else { false })
        }
        fn same(input: u32, gate: bool) -> (u32, u32, bool) {
            fn two(value: u32) -> u32 { ((value | 4u32) ^ 3u32) & ((value >> 2u32) | 7u32) }
            fn one(value: u32) -> u32 { ((value & 15u32) ^ 3u32) | ((value >> 4u32) & 31u32) }
            (one(input), two(input), gate)
        }
        fn different(input: u32, gate: bool) -> (u32, u32, bool) {
            fn one(value: u32) -> u32 { ((value & 15u32) ^ 4u32) | ((value >> 4u32) & 31u32) }
            fn two(value: u32) -> u32 { ((value | 4u32) ^ 3u32) & ((value >> 2u32) | 7u32) }
            (one(input), two(input), gate)
        }
        ",
    )
    .unwrap();
    let context = syn_canon::SourceContext::new(core::iter::once((&[][..], &source)));
    let canonical = |name: &str| {
        let function = function(&source, name);
        context
            .function(&function.sig, &function.block)
            .unwrap()
            .canonicalize()
    };
    assert_eq!(canonical("first"), canonical("same"));
    assert_ne!(canonical("first"), canonical("different"));
}

#[test]
fn borrowed_tied_local_helpers_compose_with_parameter_copies() {
    let source: syn::File = syn::parse_str(
        "
        fn copied(input: u32) -> u32 {
            fn first(value: u32) -> u32 { value }
            fn second(value: u32) -> u32 { value }
            let copy = input;
            first(copy) ^ second(input)
        }
        fn direct(input: u32) -> u32 {
            fn second(value: u32) -> u32 { value }
            fn first(value: u32) -> u32 { value }
            first(input) ^ second(input)
        }
        ",
    )
    .unwrap();
    let context = syn_canon::SourceContext::new(core::iter::once((&[][..], &source)));
    let canonical = |name: &str| {
        let function = function(&source, name);
        context
            .function(&function.sig, &function.block)
            .unwrap()
            .canonicalize()
    };
    assert_eq!(canonical("copied"), canonical("direct"));
}
