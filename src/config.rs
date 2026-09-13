//! `.dejadoc.toml` parameters. A missing file is a no-op.

use std::path::Path;

#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Report groups with at least this many sites.
    pub threshold: Option<usize>,
    /// Skip blocks with fewer tokens than this.
    pub min_tokens: Option<usize>,
}

/// Load `path`, or the default config when the file is absent.
pub fn load(path: &Path) -> anyhow::Result<Config> {
    // Implemented in the report + config + CLI phase.
    let _ = path;
    Ok(Config::default())
}
