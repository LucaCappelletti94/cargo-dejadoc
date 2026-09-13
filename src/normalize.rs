//! Canonical form of a doctest body.

/// Canonical form of a doctest body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Canonical {
    /// Canonical text: deterministic token-stream string, or the collapsed
    /// text fallback.
    pub text: String,
    /// True when no `syn` parse succeeded and the text fallback was used.
    pub unparsed: bool,
    /// Token count for `min-tokens` filtering.
    pub tokens: usize,
}

/// Compute the canonical form of a doctest body.
pub fn canonicalize(code: &str) -> Canonical {
    // Implemented in the normalize phase.
    Canonical {
        text: code.to_string(),
        unparsed: true,
        tokens: 0,
    }
}
