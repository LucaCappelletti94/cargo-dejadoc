//! Workspace, target, and module-tree discovery.

use std::path::PathBuf;

/// One compilable target to scan.
#[derive(Debug, Clone)]
pub struct Target {
    /// Crate name used as the item-path root.
    pub name: String,
    /// Target kind.
    pub kind: TargetKind,
    /// Absolute path of the target's source root file.
    pub src: PathBuf,
}

/// Kind of target, matching what `cargo test --doc` runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetKind {
    Lib,
    Bin,
    Example,
}

/// A resolved workspace: root directory and the targets to scan.
#[derive(Debug, Clone)]
pub struct Workspace {
    /// Workspace root directory.
    pub root: PathBuf,
    /// Targets to scan, in metadata order.
    pub targets: Vec<Target>,
}

/// Resolve the workspace containing `root` and the targets to scan.
pub fn workspace(
    root: &std::path::Path,
    package: Option<&str>,
    all_targets: bool,
) -> anyhow::Result<Workspace> {
    // Implemented in the discover phase.
    let _ = (root, package, all_targets);
    anyhow::bail!("discover not implemented yet")
}

/// Walk the module tree from a target root: the root file plus every
/// `mod`-declared file, with `syn` parse results.
pub fn module_tree(target: &Target) -> anyhow::Result<Vec<(PathBuf, syn::File)>> {
    // Implemented in the discover phase.
    let _ = target;
    anyhow::bail!("discover not implemented yet")
}
