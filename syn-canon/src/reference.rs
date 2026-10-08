//! Resolution provenance and conservative block proof facts.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use crate::scope::PrimTy;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum ReferenceKind {
    #[default]
    Unresolved,
    Owned,
    Inherited,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Resolution {
    pub(crate) kind: ReferenceKind,
    pub(crate) prim: Option<PrimTy>,
}

/// The statement roles, scalar types, and ordered tail types of one block.
#[derive(Clone, Debug)]
pub(crate) struct BlockProof {
    pub(crate) statements: Vec<u8>,
    pub(crate) types: Vec<Option<PrimTy>>,
    pub(crate) tail_types: Vec<Option<PrimTy>>,
}

#[derive(Default)]
pub(crate) struct ReferenceFacts {
    pub(crate) paths: BTreeMap<*const syn::Path, Resolution>,
    pub(crate) macros: BTreeMap<*const syn::Macro, Vec<Resolution>>,
    pub(crate) blocks: BTreeMap<*const syn::Block, BlockProof>,
}
