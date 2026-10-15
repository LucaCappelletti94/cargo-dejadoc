//! Resolution provenance and block scheduling facts.

use alloc::collections::BTreeMap;
use alloc::collections::BTreeSet;
use alloc::string::String;
use alloc::vec::Vec;

use crate::scope::PrimTy;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum ReferenceKind {
    #[default]
    Unresolved,
    Owned,
    Inherited,
}

/// The original binding identity a reference resolved to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ResolvedOrigin {
    pub(crate) domain: crate::scope::Domain,
    pub(crate) ns: crate::scope::Ns,
    pub(crate) id: crate::scope::BindingId,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Resolution {
    pub(crate) kind: ReferenceKind,
    pub(crate) prim: Option<PrimTy>,
    /// The original resolver's binding identity, when a binding answered.
    pub(crate) origin: Option<ResolvedOrigin>,
}

/// The one proof-backed original route of a terminal owner, every spelling
/// of the same terminal resolving to the same path through the original
/// module graph.
#[derive(Clone, Debug)]
pub(crate) struct OwnerRoute {
    /// The original segment spellings, outermost first, raw kept.
    pub(crate) segments: Vec<String>,
    /// The genuine original binding each segment names.
    pub(crate) origins: Vec<ResolvedOrigin>,
    /// The head path's retained resolution.
    pub(crate) first: Resolution,
}

/// The statement roles, scalar types, and ordered tail types of one block.
#[derive(Clone, Debug)]
pub(crate) struct BlockProof {
    pub(crate) statements: Vec<u8>,
    pub(crate) types: Vec<Option<PrimTy>>,
    pub(crate) tail_types: Vec<Option<PrimTy>>,
}

pub(crate) type MarkerOrigins<T> = BTreeMap<*const T, (crate::scope::Ns, usize)>;
pub(crate) type MarkerMacroOrigins =
    BTreeMap<*const syn::PathSegment, Vec<Option<(crate::scope::Ns, usize)>>>;

#[derive(Default)]
pub(crate) struct ReferenceFacts {
    pub(crate) paths: BTreeMap<*const syn::Path, Resolution>,
    pub(crate) macros: BTreeMap<*const syn::Macro, Vec<Resolution>>,
    pub(crate) blocks: BTreeMap<*const syn::Block, BlockProof>,
    pub(crate) idents: BTreeMap<*const proc_macro2::Ident, ResolvedOrigin>,
    pub(crate) declarations: BTreeSet<*const proc_macro2::Ident>,
    pub(crate) original_paths: BTreeMap<*const syn::PathSegment, Resolution>,
    pub(crate) original_macros: BTreeMap<*const syn::PathSegment, Vec<Resolution>>,
    pub(crate) original_calls: BTreeMap<*const syn::PathSegment, crate::scope::HelperId>,
    pub(crate) original_call_scopes: BTreeMap<*const syn::PathSegment, crate::helpers::ScopeId>,
    pub(crate) marker_paths: MarkerOrigins<syn::PathSegment>,
    pub(crate) marker_macros: MarkerMacroOrigins,
    pub(crate) original_parameters: BTreeMap<(bool, crate::scope::BindingId), usize>,
    /// The proof-backed owner route of a terminal owner, the one original
    /// path every spelling of the same terminal resolves to.
    pub(crate) owner_routes: BTreeMap<(bool, crate::scope::BindingId), OwnerRoute>,
    pub(crate) portable_paths: BTreeSet<*const syn::PathSegment>,
    pub(crate) known_macros: BTreeSet<*const syn::PathSegment>,
}

impl ReferenceFacts {
    /// Keep statement roles and scalar types aligned with a permutation step.
    pub(crate) fn swap_block(&mut self, block: &syn::Block, left: usize, right: usize) {
        let proof = self
            .blocks
            .get_mut(&core::ptr::from_ref(block))
            .expect("an analyzed block");
        proof.statements.swap(left, right);
        proof.types.swap(left, right);
    }
}
