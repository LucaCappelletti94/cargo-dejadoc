/// Reset every span of a token stream, and of each nested group, to the call site.
pub(super) fn reset_spans(stream: proc_macro2::TokenStream) -> proc_macro2::TokenStream {
    stream
        .into_iter()
        .map(|mut token| {
            if let proc_macro2::TokenTree::Group(group) = token {
                token = proc_macro2::TokenTree::Group(proc_macro2::Group::new(
                    group.delimiter(),
                    reset_spans(group.stream()),
                ));
            }
            token.set_span(proc_macro2::Span::call_site());
            token
        })
        .collect()
}
