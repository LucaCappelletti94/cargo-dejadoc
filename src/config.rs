//! `.dejadoc.toml` parameters. A missing file is a no-op.

use std::path::Path;

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Report groups with at least this many sites.
    pub threshold: Option<usize>,
    /// Skip blocks with fewer tokens than this.
    #[serde(rename = "min-tokens")]
    pub min_tokens: Option<usize>,
}

/// Load `path`, or the default config when the file is absent.
pub fn load(path: &Path) -> anyhow::Result<Config> {
    if !path.exists() {
        return Ok(Config::default());
    }
    let text = std::fs::read_to_string(path)?;
    Ok(toml::from_str(&text)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_is_default() {
        let cfg = load(Path::new("/nonexistent/.dejadoc.toml")).unwrap();
        assert_eq!(cfg, Config::default());
    }

    #[test]
    fn loads_parameters() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".dejadoc.toml");
        std::fs::write(&path, "threshold = 3\nmin-tokens = 4\n").unwrap();
        let cfg = load(&path).unwrap();
        assert_eq!(cfg.threshold, Some(3));
        assert_eq!(cfg.min_tokens, Some(4));
    }

    #[test]
    fn partial_file_keeps_defaults_for_absent_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".dejadoc.toml");
        std::fs::write(&path, "threshold = 3\n").unwrap();
        let cfg = load(&path).unwrap();
        assert_eq!(cfg.threshold, Some(3));
        assert_eq!(cfg.min_tokens, None);
    }

    #[test]
    fn unknown_field_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".dejadoc.toml");
        std::fs::write(&path, "bogus = 1\n").unwrap();
        assert!(load(&path).is_err());
    }

    #[test]
    fn invalid_toml_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".dejadoc.toml");
        std::fs::write(&path, "threshold = ").unwrap();
        assert!(load(&path).is_err());
    }
}
