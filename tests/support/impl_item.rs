/// The named inherent method of one file.
pub(super) fn impl_method<'a>(source: &'a syn::File, name: &str) -> &'a syn::ImplItemFn {
    source
        .items
        .iter()
        .find_map(|item| match item {
            syn::Item::Impl(item) => item.items.iter().find_map(|member| match member {
                syn::ImplItem::Fn(function) if function.sig.ident == name => Some(function),
                _ => None,
            }),
            _ => None,
        })
        .unwrap()
}
