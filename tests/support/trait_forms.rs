use syn_canon::{CanonicalForm, SourceContext};

/// The canonical forms of one trait's default method bodies.
pub(super) fn trait_method_forms(
    context: &SourceContext<'_>,
    trait_: &syn::ItemTrait,
) -> Vec<CanonicalForm> {
    trait_
        .items
        .iter()
        .filter_map(|item| match item {
            syn::TraitItem::Fn(method) => Some(
                context
                    .function(&method.sig, method.default.as_ref().unwrap())
                    .unwrap()
                    .canonicalize(),
            ),
            _ => None,
        })
        .collect()
}
