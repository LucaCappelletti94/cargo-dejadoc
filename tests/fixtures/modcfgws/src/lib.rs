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
