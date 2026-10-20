use dejadoc::{Kind, Report};

/// The sorted ids of the function groups of one report.
pub(super) fn function_ids(report: &Report) -> Vec<&str> {
    let mut ids: Vec<&str> = report
        .groups
        .iter()
        .filter(|group| group.kind == Kind::Function)
        .map(|group| group.id.as_str())
        .collect();
    ids.sort_unstable();
    ids
}
