//! `.dejadoc.toml` parameters. A missing file is a no-op.

use std::path::Path;

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
/// Parameters from a `.dejadoc.toml` file.
pub(crate) struct Config {
    /// Report groups with at least this many sites.
    pub(crate) threshold: Option<usize>,
    /// Skip blocks with fewer tokens than this.
    #[serde(rename = "min-tokens")]
    pub(crate) min_tokens: Option<usize>,
    /// Whether the duplicate function check runs.
    pub(crate) functions: Option<bool>,
    /// Skip functions with fewer tokens than this.
    #[serde(rename = "fn-min-tokens")]
    pub(crate) fn_min_tokens: Option<usize>,
    /// Scan files marked generated as well.
    #[serde(rename = "scan-generated")]
    pub(crate) scan_generated: Option<bool>,
    /// More phrases that mark a file generated in its first lines.
    #[serde(rename = "generated-markers")]
    pub(crate) generated_markers: Option<alloc::vec::Vec<alloc::string::String>>,
}

/// Load `path`, or the default config when the file is absent.
///
/// # Errors
///
/// Fails when the file exists but cannot be read or is not valid TOML.
pub(crate) fn load(path: &Path) -> crate::Result<Config> {
    if !path.exists() {
        return Ok(Config::default());
    }
    let text = std::fs::read_to_string(path)?;
    Ok(toml::from_str(&text)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;

    #[test]
    fn missing_file_is_default() {
        let cfg = load(Path::new("/nonexistent/.dejadoc.toml")).unwrap();
        assert_eq!(cfg, Config::default());
    }

    #[test]
    fn loads_parameters() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".dejadoc.toml");
        std::fs::write(
            &path,
            "threshold = 3\nmin-tokens = 4\nfunctions = false\nfn-min-tokens = 50\nscan-generated = true\ngenerated-markers = [\"by rust-bindgen\"]\n",
        )
        .unwrap();
        let cfg = load(&path).unwrap();
        assert_eq!(cfg.threshold, Some(3));
        assert_eq!(cfg.min_tokens, Some(4));
        assert_eq!(cfg.functions, Some(false));
        assert_eq!(cfg.fn_min_tokens, Some(50));
        assert_eq!(cfg.scan_generated, Some(true));
        assert_eq!(
            cfg.generated_markers,
            Some(alloc::vec!["by rust-bindgen".to_string()])
        );
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
