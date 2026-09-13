//! Report rendering.

use crate::Report;

/// Render the report for humans.
pub fn human(report: &Report, verbose: bool) -> String {
    // Implemented in the report phase.
    let _ = (report, verbose);
    String::new()
}

/// Render the report as JSON.
pub fn json(report: &Report) -> String {
    // Implemented in the report phase.
    let _ = report;
    String::new()
}
