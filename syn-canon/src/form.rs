//! Framed token identity and size metrics.

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt::Write;
use proc_macro2::{Group, Ident, Spacing, TokenStream, TokenTree};
use quote::ToTokens;
use syn::visit::{self, Visit};
use syn::visit_mut::{self, VisitMut};

use crate::reference::{ReferenceFacts, ReferenceKind, Resolution};
use crate::scope::PrimTy;

/// The label prefix of an owner parameter in a method view, followed by its role or number.
pub(crate) const OWNER_PARAMETER: &str = "__dejadoc_owner_param_";

/// An opaque comparison key with canonical leaf and body sizes.
pub struct CanonicalForm {
    key: String,
    /// The key with owner parameters labelled by role, when the view uses an owner parameter.
    owner_roles: Option<String>,
    leaf_tokens: usize,
    body_units: usize,
}

impl CanonicalForm {
    pub(crate) fn from_tokens(tokens: TokenStream, body_units: usize) -> Self {
        let mut key = String::from("K1 S ");
        let mut leaf_tokens = 0;
        let mut scratch = String::new();
        for tree in tokens {
            framed_tree(&mut key, tree, &mut leaf_tokens, &mut scratch);
        }
        key.push('E');
        Self {
            key,
            owner_roles: None,
            leaf_tokens,
            body_units,
        }
    }

    /// The opaque comparison key.
    #[must_use]
    pub fn key(&self) -> &str {
        &self.key
    }

    /// The key of a method view with its owner's parameters labelled by their role in the owner
    /// type, when the view uses one. `key` numbers them by first use instead.
    #[must_use]
    pub fn owner_role_key(&self) -> Option<&str> {
        self.owner_roles.as_deref()
    }

    /// The owned comparison key.
    #[must_use]
    pub fn into_key(self) -> String {
        self.key
    }

    /// Canonical leaf tokens, excluding framing.
    #[must_use]
    pub fn leaf_tokens(&self) -> usize {
        self.leaf_tokens
    }

    /// Canonical body units, excluding framing.
    #[must_use]
    pub fn body_units(&self) -> usize {
        self.body_units
    }
}

pub(crate) trait CanonicalNode: ToTokens {
    fn visit<'ast>(&'ast self, visitor: &mut impl Visit<'ast>);

    fn canonical_body_units(&self, tokens: &TokenStream) -> usize {
        body_units(tokens.clone())
    }
}

impl CanonicalNode for syn::File {
    fn visit<'ast>(&'ast self, visitor: &mut impl Visit<'ast>) {
        visitor.visit_file(self);
    }

    fn canonical_body_units(&self, tokens: &TokenStream) -> usize {
        if !matches!(self.items.as_slice(), [syn::Item::Fn(_)]) {
            return 0;
        }
        match tokens.clone().into_iter().last() {
            Some(TokenTree::Group(body)) => body_units(body.stream()),
            _ => 0,
        }
    }
}

impl CanonicalNode for syn::Block {
    fn visit<'ast>(&'ast self, visitor: &mut impl Visit<'ast>) {
        visitor.visit_block(self);
    }
}

impl CanonicalNode for syn::Expr {
    fn visit<'ast>(&'ast self, visitor: &mut impl Visit<'ast>) {
        visitor.visit_expr(self);
    }
}

impl CanonicalNode for syn::Arm {
    fn visit<'ast>(&'ast self, visitor: &mut impl Visit<'ast>) {
        visitor.visit_arm(self);
    }
}

pub(crate) fn canonical_file_with_options(
    file: syn::File,
    compiles: bool,
    options: crate::alpha::Options<'_>,
) -> CanonicalForm {
    canonical_file(file, compiles, options, true).0
}

/// The original proof backing of one item's comparison projection.
pub(crate) struct PayloadProof<'a> {
    pub(crate) table: Option<&'a crate::helpers::HelperTable>,
    pub(crate) frame: &'a crate::scope::Frame,
    pub(crate) syntax: crate::helpers::HelperSyntax,
    pub(crate) edges: MarkerEdges,
}

/// The per-occurrence group references of one item's marker file, each
/// substitution keeping the origin it replaced.
pub(crate) struct MarkerEdges {
    idents: crate::reference::MarkerOrigins<proc_macro2::Ident>,
    macros: crate::reference::MarkerMacroOrigins,
}

impl MarkerEdges {
    pub(crate) fn new() -> Self {
        Self {
            idents: alloc::collections::BTreeMap::new(),
            macros: alloc::collections::BTreeMap::new(),
        }
    }

    pub(crate) fn record_ident(
        &mut self,
        ident: &proc_macro2::Ident,
        ns: crate::scope::Ns,
        slot: usize,
    ) {
        self.idents.insert(core::ptr::from_ref(ident), (ns, slot));
    }

    pub(crate) fn record_macro(
        &mut self,
        mac: &syn::Macro,
        occurrences: Vec<Option<(crate::scope::Ns, usize)>>,
    ) {
        let Some(first) = mac.path.segments.first() else {
            return;
        };
        self.macros.insert(core::ptr::from_ref(first), occurrences);
    }

    pub(crate) fn into_parts(
        self,
    ) -> (
        crate::reference::MarkerOrigins<proc_macro2::Ident>,
        crate::reference::MarkerMacroOrigins,
    ) {
        (self.idents, self.macros)
    }
}

struct Walker<'a> {
    facts: &'a ReferenceFacts,
    out: &'a mut Vec<(crate::scope::Ns, usize)>,
}

impl<'ast> Visit<'ast> for Walker<'_> {
    fn visit_path(&mut self, path: &'ast syn::Path) {
        if let Some(last) = path.segments.last()
            && let Some((ns, slot)) = self.facts.marker_paths.get(&core::ptr::from_ref(last))
        {
            self.out.push((*ns, *slot));
        }
        visit::visit_path(self, path);
    }

    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        if let Some(first) = mac.path.segments.first()
            && let Some(records) = self.facts.marker_macros.get(&core::ptr::from_ref(first))
        {
            for record in records.iter().flatten() {
                self.out.push(*record);
            }
        }
        visit::visit_macro(self, mac);
    }
}

pub(crate) fn item_payload(
    file: syn::File,
    proof: PayloadProof<'_>,
) -> (CanonicalForm, Vec<(crate::scope::Ns, usize)>) {
    let mut syntax = proof.syntax;
    let (idents, macros) = proof.edges.into_parts();
    syntax.ingest_marker_edges(&file, idents, macros);
    let (form, normalized, facts) = canonical_file(
        file,
        true,
        crate::alpha::Options {
            seed: Some(proof.frame),
            generics: None,
            self_owner: None,
            prim_fallback: true,
            observed: false,
            helper_table: proof.table,
            helper_module: &[],
            helper_blocked: false,
            helper_syntax: Some(syntax),
        },
        false,
    );
    let mut edges = Vec::new();
    let mut walker = Walker {
        facts: &facts,
        out: &mut edges,
    };
    visit::visit_file(&mut walker, &normalized);
    (form, edges)
}

fn canonical_file(
    mut file: syn::File,
    compiles: bool,
    mut options: crate::alpha::Options<'_>,
    items: bool,
) -> (CanonicalForm, syn::File, ReferenceFacts) {
    let owner = options.generics.is_some();
    if owner {
        ReservedLabels.visit_file_mut(&mut file);
    }
    let helpers = (compiles && items && options.helper_table.is_none())
        .then(|| crate::helpers::module_table(&crate::helpers::Scopes::from_file(&file)));
    let helper_table = options.helper_table.or(helpers.as_ref());
    let mut helper_syntax = options.helper_syntax.take().unwrap_or_else(|| {
        helper_table.map_or_else(crate::helpers::HelperSyntax::default, |table| {
            crate::helpers::HelperSyntax::from_file(table, &mut file)
        })
    });
    if compiles && items {
        crate::items::normalize(
            &mut file,
            &options,
            &mut helper_syntax,
            helper_table.expect("items normalization has a helper table"),
        );
    }
    crate::drift::normalize_file(&mut file, compiles, &mut helper_syntax);
    let options = crate::alpha::Options {
        helper_table,
        helper_syntax: Some(helper_syntax),
        ..options
    };
    let facts = crate::alpha::normalize_file(&mut file, compiles, options);
    (from_node_with(&file, &facts, owner), file, facts)
}

pub(crate) fn from_node(node: &impl CanonicalNode, facts: &ReferenceFacts) -> CanonicalForm {
    from_node_with(node, facts, false)
}

/// The comparison form of `node`. With `owner`, the key numbers owner parameters by first
/// appearance, and the role-labelled tokens give the owner role key.
fn from_node_with(node: &impl CanonicalNode, facts: &ReferenceFacts, owner: bool) -> CanonicalForm {
    let tokens: TokenStream = crate::fold_tokens(node.to_token_stream(), false, false)
        .into_iter()
        .collect();
    let units = node.canonical_body_units(&tokens);
    let mut tail = String::from("R ");
    node.visit(&mut ReferenceEncoding {
        out: &mut tail,
        facts,
    });
    tail.push('E');
    let finish = |tokens: TokenStream| {
        let mut form = CanonicalForm::from_tokens(tokens, units);
        form.key.pop();
        form.key.push_str(&tail);
        form
    };
    if !owner {
        return finish(tokens);
    }
    let mut ranks = BTreeMap::new();
    let numbered = number_owner_parameters(tokens.clone(), &mut ranks);
    if ranks.is_empty() {
        return finish(tokens);
    }
    let roles = finish(tokens);
    let mut form = finish(numbered);
    form.owner_roles = Some(roles.key);
    form
}

/// Rename every owner parameter label in `tokens` to its rank among the labels seen so far.
fn number_owner_parameters(
    tokens: TokenStream,
    ranks: &mut BTreeMap<String, usize>,
) -> TokenStream {
    tokens
        .into_iter()
        .map(|tree| match tree {
            TokenTree::Ident(ident) if ident.to_string().starts_with(OWNER_PARAMETER) => {
                let next = ranks.len();
                let rank = *ranks.entry(ident.to_string()).or_insert(next);
                TokenTree::Ident(Ident::new(
                    &alloc::format!("{OWNER_PARAMETER}{rank}"),
                    ident.span(),
                ))
            }
            TokenTree::Group(group) => {
                let mut renamed = Group::new(
                    group.delimiter(),
                    number_owner_parameters(group.stream(), ranks),
                );
                renamed.set_span(group.span());
                TokenTree::Group(renamed)
            }
            other => other,
        })
        .collect()
}

/// The prefix that keeps a source identifier apart from every generated owner parameter label.
const ESCAPED: &str = "__dejadoc_escaped_";

/// Respells source identifiers that start with [`OWNER_PARAMETER`] or [`ESCAPED`] under
/// [`ESCAPED`], macro and attribute tokens included, so every owner parameter label left after
/// renaming comes from an owner parameter. Raw identifiers keep their `r#` and never read as
/// labels.
struct ReservedLabels;

impl ReservedLabels {
    fn ident(ident: &Ident) -> Option<Ident> {
        let name = ident.to_string();
        (name.starts_with(OWNER_PARAMETER) || name.starts_with(ESCAPED))
            .then(|| Ident::new(&alloc::format!("{ESCAPED}{name}"), ident.span()))
    }

    fn tokens(tokens: TokenStream) -> TokenStream {
        tokens
            .into_iter()
            .map(|tree| match tree {
                TokenTree::Ident(ident) => TokenTree::Ident(Self::ident(&ident).unwrap_or(ident)),
                TokenTree::Group(group) => {
                    let mut escaped = Group::new(group.delimiter(), Self::tokens(group.stream()));
                    escaped.set_span(group.span());
                    TokenTree::Group(escaped)
                }
                other => other,
            })
            .collect()
    }
}

impl VisitMut for ReservedLabels {
    fn visit_ident_mut(&mut self, ident: &mut Ident) {
        if let Some(escaped) = Self::ident(ident) {
            *ident = escaped;
        }
    }

    fn visit_macro_mut(&mut self, mac: &mut syn::Macro) {
        visit_mut::visit_macro_mut(self, mac);
        mac.tokens = Self::tokens(core::mem::take(&mut mac.tokens));
    }

    fn visit_meta_list_mut(&mut self, list: &mut syn::MetaList) {
        visit_mut::visit_meta_list_mut(self, list);
        list.tokens = Self::tokens(core::mem::take(&mut list.tokens));
    }

    fn visit_expr_mut(&mut self, expr: &mut syn::Expr) {
        match expr {
            syn::Expr::Verbatim(tokens) => *tokens = Self::tokens(core::mem::take(tokens)),
            other => visit_mut::visit_expr_mut(self, other),
        }
    }

    fn visit_type_mut(&mut self, ty: &mut syn::Type) {
        match ty {
            syn::Type::Verbatim(tokens) => *tokens = Self::tokens(core::mem::take(tokens)),
            other => visit_mut::visit_type_mut(self, other),
        }
    }

    fn visit_pat_mut(&mut self, pat: &mut syn::Pat) {
        match pat {
            syn::Pat::Verbatim(tokens) => *tokens = Self::tokens(core::mem::take(tokens)),
            other => visit_mut::visit_pat_mut(self, other),
        }
    }

    fn visit_item_mut(&mut self, item: &mut syn::Item) {
        match item {
            syn::Item::Verbatim(tokens) => *tokens = Self::tokens(core::mem::take(tokens)),
            other => visit_mut::visit_item_mut(self, other),
        }
    }

    fn visit_impl_item_mut(&mut self, item: &mut syn::ImplItem) {
        match item {
            syn::ImplItem::Verbatim(tokens) => *tokens = Self::tokens(core::mem::take(tokens)),
            other => visit_mut::visit_impl_item_mut(self, other),
        }
    }

    fn visit_trait_item_mut(&mut self, item: &mut syn::TraitItem) {
        match item {
            syn::TraitItem::Verbatim(tokens) => *tokens = Self::tokens(core::mem::take(tokens)),
            other => visit_mut::visit_trait_item_mut(self, other),
        }
    }
}

struct ReferenceEncoding<'a> {
    out: &'a mut String,
    facts: &'a ReferenceFacts,
}

impl ReferenceEncoding<'_> {
    fn resolution(&mut self, resolution: Resolution) {
        self.out.push('r');
        self.out.push(match resolution.kind {
            ReferenceKind::Unresolved => '0',
            ReferenceKind::Owned => '1',
            ReferenceKind::Inherited => '2',
        });
        self.out.push(' ');
        self.primitive(resolution.prim);
    }

    fn primitive(&mut self, primitive: Option<PrimTy>) {
        self.out.push('t');
        self.out.push_str(primitive.map_or("_", PrimTy::suffix));
        self.out.push(' ');
    }
}

impl<'ast> Visit<'ast> for ReferenceEncoding<'_> {
    fn visit_block(&mut self, block: &'ast syn::Block) {
        if let Some(proof) = self.facts.blocks.get(&core::ptr::from_ref(block)) {
            write!(self.out, "b {} ", proof.statements.len())
                .expect("writing to a string succeeds");
            for (&role, &primitive) in proof.statements.iter().zip(&proof.types) {
                self.out.push(char::from(role));
                self.primitive(primitive);
            }
            write!(self.out, "o {} ", proof.tail_types.len())
                .expect("writing to a string succeeds");
            for &primitive in &proof.tail_types {
                self.primitive(primitive);
            }
        }
        visit::visit_block(self, block);
    }

    fn visit_path(&mut self, path: &'ast syn::Path) {
        self.resolution(
            self.facts
                .paths
                .get(&core::ptr::from_ref(path))
                .copied()
                .unwrap_or_default(),
        );
        visit::visit_path(self, path);
    }

    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        let resolutions = self
            .facts
            .macros
            .get(&core::ptr::from_ref(mac))
            .map_or(&[][..], alloc::vec::Vec::as_slice);
        write!(self.out, "m {} ", resolutions.len()).expect("writing to a string succeeds");
        for &resolution in resolutions {
            self.resolution(resolution);
        }
        visit::visit_macro(self, mac);
    }
}

impl core::fmt::Display for CanonicalForm {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.key)
    }
}

impl core::fmt::Debug for CanonicalForm {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_tuple("CanonicalForm").field(&self.key).finish()
    }
}

impl PartialEq for CanonicalForm {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}

impl Eq for CanonicalForm {}

impl core::hash::Hash for CanonicalForm {
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        core::hash::Hash::hash(&self.key, state);
    }
}

fn framed_tree(out: &mut String, tree: TokenTree, leaves: &mut usize, scratch: &mut String) {
    match tree {
        TokenTree::Ident(ident) => {
            scratch.clear();
            write!(scratch, "{ident}").expect("writing to a string succeeds");
            framed_payload(out, 'i', scratch);
            *leaves += 1;
        }
        TokenTree::Literal(literal) => {
            scratch.clear();
            write!(scratch, "{literal}").expect("writing to a string succeeds");
            framed_payload(out, 'l', scratch);
            *leaves += 1;
        }
        TokenTree::Punct(punct) => {
            out.push('p');
            out.push(if punct.spacing() == Spacing::Joint {
                'j'
            } else {
                'a'
            });
            out.push(punct.as_char());
            out.push(' ');
            *leaves += 1;
        }
        TokenTree::Group(group) => {
            out.push('g');
            out.push(match group.delimiter() {
                proc_macro2::Delimiter::None => 'n',
                proc_macro2::Delimiter::Parenthesis => '(',
                proc_macro2::Delimiter::Bracket => '[',
                proc_macro2::Delimiter::Brace => '{',
            });
            out.push(' ');
            for tree in group.stream() {
                framed_tree(out, tree, leaves, scratch);
            }
            out.push_str("e ");
        }
    }
}

fn framed_payload(out: &mut String, kind: char, payload: &str) {
    write!(out, "{kind} {} {payload} ", payload.len()).expect("writing to a string succeeds");
}

pub(crate) fn body_units(stream: TokenStream) -> usize {
    let mut trees = stream.into_iter().peekable();
    let mut count = 0;
    let (mut ident, mut joined) = (false, false);
    while let Some(tree) = trees.next() {
        match tree {
            TokenTree::Group(group) => {
                count += body_units(group.stream());
                (ident, joined) = (false, false);
            }
            TokenTree::Ident(_) => {
                count += usize::from(!joined);
                (ident, joined) = (true, false);
            }
            TokenTree::Literal(_) => {
                count += 1;
                (ident, joined) = (false, false);
            }
            TokenTree::Punct(first) => {
                if first.as_char() == '\'' && matches!(trees.peek(), Some(TokenTree::Ident(_))) {
                    trees.next();
                    count += 1;
                    (ident, joined) = (false, false);
                    continue;
                }
                let c = first.as_char();
                let second = match trees.peek() {
                    Some(TokenTree::Punct(second)) if first.spacing() == Spacing::Joint => {
                        Some((second.as_char(), second.spacing()))
                    }
                    _ => None,
                };
                if second.is_some_and(|(second, _)| c == ':' && second == ':') {
                    trees.next();
                    joined = ident;
                    continue;
                }
                if let Some((second, spacing)) = second
                    && OPERATORS.iter().any(|op| {
                        let bytes = op.as_bytes();
                        char::from(bytes[0]) == c && char::from(bytes[1]) == second
                    })
                {
                    trees.next();
                    if spacing == Spacing::Joint
                        && let Some(TokenTree::Punct(third)) = trees.peek()
                        && OPERATORS.iter().any(|op| {
                            let bytes = op.as_bytes();
                            bytes.len() == 3
                                && char::from(bytes[0]) == c
                                && char::from(bytes[1]) == second
                                && char::from(bytes[2]) == third.as_char()
                        })
                    {
                        trees.next();
                    }
                }
                count += usize::from(c != ',');
                (ident, joined) = (false, false);
            }
        }
    }
    count
}

const OPERATORS: [&str; 23] = [
    "<<=", ">>=", "...", "..=", "=>", "==", "!=", "<=", ">=", "&&", "||", "+=", "-=", "*=", "/=",
    "%=", "^=", "&=", "|=", "<<", ">>", "..", "->",
];
