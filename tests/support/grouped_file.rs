/// Parse a source file and wrap every expression in a delimiterless group.
pub(super) fn grouped_file(source: &str) -> syn::File {
    struct Groups;

    impl syn::visit_mut::VisitMut for Groups {
        fn visit_expr_mut(&mut self, expr: &mut syn::Expr) {
            syn::visit_mut::visit_expr_mut(self, expr);
            let inner =
                core::mem::replace(expr, syn::Expr::Verbatim(proc_macro2::TokenStream::new()));
            *expr = syn::Expr::Group(syn::ExprGroup {
                attrs: Vec::new(),
                group_token: syn::token::Group::default(),
                expr: Box::new(inner),
            });
        }
    }

    let mut file: syn::File = syn::parse_str(source).unwrap();
    syn::visit_mut::VisitMut::visit_file_mut(&mut Groups, &mut file);
    file
}
