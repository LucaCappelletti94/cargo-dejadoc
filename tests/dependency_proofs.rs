//! Conservative primitive facts in original function contexts.

use syn_canon::SourceContext;

fn function<'a>(file: &'a syn::File, name: &str) -> &'a syn::ItemFn {
    file.items
        .iter()
        .find_map(|item| match item {
            syn::Item::Fn(function) if function.sig.ident == name => Some(function),
            _ => None,
        })
        .unwrap()
}

#[test]
fn identical_alias_spelling_retains_resolved_operation_width() {
    let body = "fn f(input: Word) -> (Word, Word) {
        let low = input & 15; let high = input >> 4; (low, high)
    }";
    let narrow: syn::File = syn::parse_str(&format!("type Word = u32; {body}")).unwrap();
    let wide: syn::File = syn::parse_str(&format!("type Word = u64; {body}")).unwrap();
    let narrow_context = SourceContext::new(core::iter::once((&[][..], &narrow)));
    let wide_context = SourceContext::new(core::iter::once((&[][..], &wide)));
    let narrow = function(&narrow, "f");
    let wide = function(&wide, "f");
    assert_ne!(
        narrow_context
            .function(&narrow.sig, &narrow.block)
            .unwrap()
            .canonicalize(),
        wide_context
            .function(&wide.sig, &wide.block)
            .unwrap()
            .canonicalize(),
    );
}

#[test]
fn identical_alias_spelling_retains_resolved_tail_type() {
    let narrow: syn::File =
        syn::parse_str("type Word = u32; fn f(input: Word) -> Word { input }").unwrap();
    let wide: syn::File =
        syn::parse_str("type Word = u64; fn f(input: Word) -> Word { input }").unwrap();
    let narrow_context = SourceContext::new(core::iter::once((&[][..], &narrow)));
    let wide_context = SourceContext::new(core::iter::once((&[][..], &wide)));
    let narrow = function(&narrow, "f");
    let wide = function(&wide, "f");
    assert_ne!(
        narrow_context
            .function(&narrow.sig, &narrow.block)
            .unwrap()
            .canonicalize(),
        wide_context
            .function(&wide.sig, &wide.block)
            .unwrap()
            .canonicalize(),
    );
}

#[test]
fn imported_alias_sharing_a_value_name_retains_its_operation_width() {
    let source = |width| {
        syn::parse_str::<syn::File>(&format!(
            "mod definitions {{ pub type Word = u{width}; pub fn Word() -> bool {{ true }} }}
            use definitions::Word;
            fn f(input: Word) -> (Word, Word) {{
                let low = input & 15; let high = input >> 4; (low, high)
            }}"
        ))
        .unwrap()
    };
    let narrow = source(32);
    let wide = source(64);
    let narrow_context = SourceContext::new([(&[][..], &narrow)]);
    let wide_context = SourceContext::new([(&[][..], &wide)]);
    let narrow = function(&narrow, "f");
    let wide = function(&wide, "f");
    assert_ne!(
        narrow_context
            .function(&narrow.sig, &narrow.block)
            .unwrap()
            .canonicalize(),
        wide_context
            .function(&wide.sig, &wide.block)
            .unwrap()
            .canonicalize(),
    );
}
