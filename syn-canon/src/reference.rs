//! Resolution provenance retained through normalization.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Resolution {
    #[default]
    Unresolved,
    Owned,
    Inherited,
}

#[derive(Default)]
pub(crate) struct ReferenceFacts {
    pub(crate) paths: BTreeMap<*const syn::Path, Resolution>,
    pub(crate) macros: BTreeMap<*const syn::Macro, Vec<Resolution>>,
}
