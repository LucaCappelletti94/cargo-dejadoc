use syn_canon::{CanonicalForm, SourceContext};

/// The canonical view of the named inherent method of one file.
pub(super) fn method_form(
    context: &SourceContext<'_>,
    source: &syn::File,
    name: &str,
) -> CanonicalForm {
    source
        .items
        .iter()
        .find_map(|item| match item {
            syn::Item::Impl(impl_item) => impl_item.items.iter().find_map(|member| match member {
                syn::ImplItem::Fn(function) if function.sig.ident == name => Some(
                    context
                        .function(&function.sig, &function.block)
                        .unwrap()
                        .canonicalize(),
                ),
                _ => None,
            }),
            _ => None,
        })
        .unwrap()
}
