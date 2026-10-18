//! Function copies that module declarations select per target.

#[cfg(target_pointer_width = "64")]
#[path = "wide/bits.rs"]
mod bits;
#[cfg(not(target_pointer_width = "64"))]
#[path = "narrow/bits.rs"]
mod bits;

#[cfg(unix)]
mod host {
    pub mod probe;
}
#[cfg(not(unix))]
mod host {
    #[path = "other_probe.rs"]
    pub mod probe;
}

#[cfg_attr(feature = "fast", path = "fast/width.rs")]
#[cfg_attr(not(feature = "fast"), path = "slow/width.rs")]
mod width;

#[path = "kept/tail.rs"]
#[cfg_attr(unix, path = "dead/tail.rs")]
mod tail;
