//! Organization source provenance and coverage.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

/// Organization source provenance and analysis coverage.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct OrganizationReport {
    /// GitHub organization login.
    pub name: String,
    /// Repositories accessible during enumeration.
    pub repositories: Vec<OrganizationRepository>,
    /// Cargo packages discovered in repository snapshots.
    pub packages: Vec<OrganizationPackage>,
    /// Package indices for each physical file present in findings.
    pub file_packages: BTreeMap<String, Vec<usize>>,
    /// Whether the accessible corpus was fully analyzed.
    pub coverage: OrganizationCoverage,
}

/// A repository and its pinned default-branch revision.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct OrganizationRepository {
    /// GitHub owner and repository name.
    pub name: String,
    /// Full commit identifier, absent for an empty or unresolved repository.
    pub commit: Option<String>,
}

/// A Cargo package in an organization snapshot.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct OrganizationPackage {
    /// Index into the organization's repository records.
    pub repository: usize,
    /// Cargo package name.
    pub name: String,
    /// Repository-relative manifest path.
    pub manifest: String,
}

/// Completeness of the accessible organization corpus.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct OrganizationCoverage {
    /// Every enumerated source was analyzed successfully.
    pub complete: bool,
    /// Failures preventing complete coverage.
    pub failures: Vec<String>,
}

#[cfg(feature = "std")]
pub(crate) fn path_label(path: &std::path::Path) -> String {
    #[cfg(windows)]
    {
        path.to_string_lossy().replace('\\', "/")
    }
    #[cfg(not(windows))]
    {
        path.to_string_lossy().into_owned()
    }
}
