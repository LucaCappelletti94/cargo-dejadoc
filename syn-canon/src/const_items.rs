//! Guarded substitution of constants used only as array lengths.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use proc_macro2::{Ident, TokenStream, TokenTree};
use syn::visit::Visit;
use syn::visit_mut::VisitMut;

pub(crate) fn inline_array_constants(file: &mut syn::File) {
    let mut analysis = Analysis::default();
    analysis.visit_file(file);
    if analysis.hazard || analysis.candidates.is_empty() {
        return;
    }
    Occurrences {
        candidates: &mut analysis.candidates,
    }
    .visit_file(file);
    analysis
        .candidates
        .retain(|candidate| candidate.uses > 0 && !candidate.rejected);
    if !analysis.candidates.is_empty() {
        Inline {
            candidates: &analysis.candidates,
        }
        .visit_file_mut(file);
    }
}

#[derive(Default)]
struct Analysis {
    hazard: bool,
    candidates: Vec<Candidate>,
}

struct Candidate {
    node: *const syn::ItemConst,
    name: String,
    raw: String,
    literal: syn::LitInt,
    uses: usize,
    rejected: bool,
}

impl Candidate {
    fn matches(&self, ident: &Ident) -> bool {
        ident == self.name.as_str() || ident == self.raw.as_str()
    }

    fn captured(&self, text: &str) -> bool {
        text.match_indices(&self.name).any(|(offset, _)| {
            let before = &text[..offset];
            let after = &text[offset + self.name.len()..];
            after.starts_with('$')
                || ((after.starts_with('}') || after.starts_with(':'))
                    && (before.ends_with('{') || before.ends_with("{r#")))
        })
    }
}

impl<'ast> Visit<'ast> for Analysis {
    fn visit_attribute(&mut self, attr: &'ast syn::Attribute) {
        let attrs = core::slice::from_ref(attr);
        self.hazard |=
            crate::drift::has_macro_attribute(attrs) || crate::drift::has_macro_derive(attrs);
        syn::visit::visit_attribute(self, attr);
    }

    fn visit_item(&mut self, item: &'ast syn::Item) {
        match item {
            syn::Item::Use(_) | syn::Item::ExternCrate(_) | syn::Item::Verbatim(_) => {
                self.hazard = true;
            }
            syn::Item::Mod(module) if module.content.is_none() => self.hazard = true,
            syn::Item::Macro(mac) if mac.ident.is_some() => self.hazard = true,
            syn::Item::Const(item) if !self.hazard => {
                if let Some(literal) = eligible(item) {
                    let name = item.ident.to_string();
                    if !name.starts_with("r#") {
                        self.candidates.push(Candidate {
                            node: item,
                            raw: format!("r#{name}"),
                            name,
                            literal: literal.clone(),
                            uses: 0,
                            rejected: false,
                        });
                    }
                }
            }
            _ => {}
        }
        syn::visit::visit_item(self, item);
    }

    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        self.hazard |= mac
            .path
            .get_ident()
            .is_none_or(|ident| !crate::COMMA_BLIND_MACROS.iter().any(|name| ident == name));
        syn::visit::visit_macro(self, mac);
    }

    fn visit_expr(&mut self, expr: &'ast syn::Expr) {
        self.hazard |= matches!(expr, syn::Expr::Verbatim(_));
        syn::visit::visit_expr(self, expr);
    }

    fn visit_type(&mut self, ty: &'ast syn::Type) {
        self.hazard |= matches!(ty, syn::Type::Verbatim(_));
        syn::visit::visit_type(self, ty);
    }

    fn visit_pat(&mut self, pat: &'ast syn::Pat) {
        self.hazard |= matches!(pat, syn::Pat::Verbatim(_));
        syn::visit::visit_pat(self, pat);
    }

    fn visit_lit(&mut self, lit: &'ast syn::Lit) {
        self.hazard |= matches!(lit, syn::Lit::Verbatim(_));
        syn::visit::visit_lit(self, lit);
    }

    fn visit_foreign_item(&mut self, item: &'ast syn::ForeignItem) {
        self.hazard |= matches!(item, syn::ForeignItem::Verbatim(_));
        syn::visit::visit_foreign_item(self, item);
    }

    fn visit_trait_item(&mut self, item: &'ast syn::TraitItem) {
        self.hazard |= matches!(item, syn::TraitItem::Verbatim(_));
        syn::visit::visit_trait_item(self, item);
    }

    fn visit_impl_item(&mut self, item: &'ast syn::ImplItem) {
        self.hazard |= matches!(item, syn::ImplItem::Verbatim(_));
        syn::visit::visit_impl_item(self, item);
    }

    fn visit_type_param_bound(&mut self, bound: &'ast syn::TypeParamBound) {
        self.hazard |= matches!(bound, syn::TypeParamBound::Verbatim(_));
        syn::visit::visit_type_param_bound(self, bound);
    }
}

fn eligible(item: &syn::ItemConst) -> Option<&syn::LitInt> {
    if !matches!(item.vis, syn::Visibility::Inherited)
        || !item.attrs.is_empty()
        || item.modifiers.defaultness.is_some()
        || !item.generics.params.is_empty()
        || item.generics.where_clause.is_some()
    {
        return None;
    }
    let syn::Type::Path(ty) = item.ty.as_ref() else {
        return None;
    };
    let syn::Expr::Lit(expr) = item.expr.as_ref() else {
        return None;
    };
    let syn::Lit::Int(literal) = &expr.lit else {
        return None;
    };
    (ty.attrs.is_empty()
        && ty.qself.is_none()
        && ty.path.is_ident("usize")
        && expr.attrs.is_empty()
        && matches!(literal.suffix(), "" | "usize"))
    .then_some(literal)
}

fn direct_ident(expr: &syn::Expr) -> Option<&Ident> {
    match expr {
        syn::Expr::Path(path) if path.attrs.is_empty() && path.qself.is_none() => {
            path.path.get_ident()
        }
        _ => None,
    }
}

struct Occurrences<'a> {
    candidates: &'a mut [Candidate],
}

impl Occurrences<'_> {
    fn length(&mut self, length: &syn::Expr) -> bool {
        let Some(ident) = direct_ident(length) else {
            return false;
        };
        let mut accounted = false;
        for candidate in &mut *self.candidates {
            if ident == candidate.name.as_str() {
                candidate.uses += 1;
                accounted = true;
            }
        }
        accounted
    }

    fn tokens(&mut self, tokens: &TokenStream) {
        for token in tokens.clone() {
            match token {
                TokenTree::Ident(ident) => self.visit_ident(&ident),
                TokenTree::Group(group) => self.tokens(&group.stream()),
                TokenTree::Literal(literal) => {
                    if let syn::Lit::Str(literal) = syn::Lit::new(literal) {
                        let value = literal.value();
                        for candidate in &mut *self.candidates {
                            candidate.rejected |= candidate.captured(&value);
                        }
                    }
                }
                TokenTree::Punct(_) => {}
            }
        }
    }
}

impl<'ast> Visit<'ast> for Occurrences<'_> {
    fn visit_item_const(&mut self, item: &'ast syn::ItemConst) {
        for candidate in &mut *self.candidates {
            if candidate.matches(&item.ident) && !core::ptr::eq(candidate.node, item) {
                candidate.rejected = true;
            }
        }
        for attr in &item.attrs {
            self.visit_attribute(attr);
        }
        self.visit_visibility(&item.vis);
        self.visit_generics(&item.generics);
        self.visit_type(&item.ty);
        self.visit_expr(&item.expr);
    }

    fn visit_type_array(&mut self, array: &'ast syn::TypeArray) {
        if self.length(&array.len) {
            for attr in &array.attrs {
                self.visit_attribute(attr);
            }
            self.visit_type(&array.elem);
        } else {
            syn::visit::visit_type_array(self, array);
        }
    }

    fn visit_expr_repeat(&mut self, repeat: &'ast syn::ExprRepeat) {
        if self.length(&repeat.len) {
            for attr in &repeat.attrs {
                self.visit_attribute(attr);
            }
            self.visit_expr(&repeat.expr);
        } else {
            syn::visit::visit_expr_repeat(self, repeat);
        }
    }

    fn visit_ident(&mut self, ident: &'ast Ident) {
        for candidate in &mut *self.candidates {
            candidate.rejected |= candidate.matches(ident);
        }
    }

    fn visit_token_stream(&mut self, tokens: &'ast TokenStream) {
        self.tokens(tokens);
    }
}

struct Inline<'a> {
    candidates: &'a [Candidate],
}

impl Inline<'_> {
    fn consumed(&self, item: &syn::Item) -> bool {
        matches!(item, syn::Item::Const(item)
            if self.candidates.iter().any(|candidate| core::ptr::eq(candidate.node, item)))
    }

    fn length(&self, length: &mut syn::Expr) {
        let Some(ident) = direct_ident(length) else {
            return;
        };
        if let Some(candidate) = self
            .candidates
            .iter()
            .find(|candidate| ident == candidate.name.as_str())
        {
            *length = syn::Expr::Lit(syn::ExprLit {
                attrs: Vec::new(),
                lit: syn::Lit::Int(candidate.literal.clone()),
            });
        }
    }
}

impl VisitMut for Inline<'_> {
    fn visit_file_mut(&mut self, file: &mut syn::File) {
        syn::visit_mut::visit_file_mut(self, file);
        file.items.retain(|item| !self.consumed(item));
    }

    fn visit_item_mod_mut(&mut self, module: &mut syn::ItemMod) {
        syn::visit_mut::visit_item_mod_mut(self, module);
        if let Some((_, items)) = &mut module.content {
            items.retain(|item| !self.consumed(item));
        }
    }

    fn visit_block_mut(&mut self, block: &mut syn::Block) {
        syn::visit_mut::visit_block_mut(self, block);
        block
            .stmts
            .retain(|stmt| !matches!(stmt, syn::Stmt::Item(item) if self.consumed(item)));
    }

    fn visit_type_array_mut(&mut self, array: &mut syn::TypeArray) {
        self.length(&mut array.len);
        syn::visit_mut::visit_type_array_mut(self, array);
    }

    fn visit_expr_repeat_mut(&mut self, repeat: &mut syn::ExprRepeat) {
        self.length(&mut repeat.len);
        syn::visit_mut::visit_expr_repeat_mut(self, repeat);
    }
}
