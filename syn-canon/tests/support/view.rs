/// The canonical view of one indexed declaration, by its written name.
pub(super) fn view_of(file: &syn::File, name: &str) -> syn_canon::CanonicalForm {
    let context = syn_canon::SourceContext::new(core::iter::once((&[][..], file)));
    let target = file
        .items
        .iter()
        .find_map(|item| match item {
            syn::Item::Fn(f) if f.sig.ident == name => Some((&f.sig, f.block.as_ref())),
            syn::Item::Impl(imp) => imp.items.iter().find_map(|member| match member {
                syn::ImplItem::Fn(f) if f.sig.ident == name => Some((&f.sig, &f.block)),
                _ => None,
            }),
            _ => None,
        })
        .unwrap_or_else(|| panic!("{name} fixture"));
    context.function(target.0, target.1).unwrap().canonicalize()
}
