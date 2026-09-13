//! Fenced code block extraction from doc text.
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;

/// A fenced code block found in doc text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fence {
    /// Info string between the backticks and the line end.
    pub info: String,
    /// 0-based index of the opening fence line within the doc text.
    pub line: usize,
    /// Body lines joined by `'\n'`.
    pub code: String,
}

/// Scan `text` for fenced code blocks. An unterminated block at EOF is
/// emitted, matching rustdoc.
#[must_use]
pub fn scan(text: &str) -> Vec<Fence> {
    let mut out = Vec::new();
    let mut open: Option<(String, usize, Vec<String>)> = None;
    for (i, line) in text.lines().enumerate() {
        let is_fence = line.trim_start().starts_with("```");
        match open.take() {
            None if is_fence => {
                let info = line
                    .trim_start()
                    .trim_start_matches("```")
                    .trim()
                    .to_string();
                open = Some((info, i, Vec::new()));
            }
            Some(f) if is_fence => out.push(Fence {
                info: f.0,
                line: f.1,
                code: f.2.join("\n"),
            }),
            Some(mut f) => {
                f.2.push(line.to_string());
                open = Some(f);
            }
            None => {}
        }
    }
    if let Some((info, line, body)) = open {
        out.push(Fence {
            info,
            line,
            code: body.join("\n"),
        });
    }
    out
}
#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn fence(info: &str, line: usize, code: &str) -> Fence {
        Fence {
            info: info.into(),
            line,
            code: code.into(),
        }
    }

    #[test]
    fn empty_text() {
        assert_eq!(scan(""), vec![]);
    }

    #[test]
    fn plain_block() {
        let t = "intro\n```rust\nlet x = 5;\n```\nend\n";
        assert_eq!(scan(t), vec![fence("rust", 1, "let x = 5;")]);
    }

    #[test]
    fn empty_info() {
        let t = "```\nfn main() {}\n```\n";
        assert_eq!(scan(t), vec![fence("", 0, "fn main() {}")]);
    }

    #[test]
    fn indented_fence_counts() {
        let t = "    ```rust\nx\n    ```\n";
        assert_eq!(scan(t), vec![fence("rust", 0, "x")]);
    }

    #[test]
    fn tilde_does_not_toggle() {
        let t = "~~~rust\nx\n~~~\n```\nfn main() {}\n```\n";
        assert_eq!(scan(t), vec![fence("", 3, "fn main() {}")]);
    }

    #[test]
    fn unterminated_emits_at_eof() {
        let t = "```rust\nfn main() {}";
        assert_eq!(scan(t), vec![fence("rust", 0, "fn main() {}")]);
    }

    #[test]
    fn fence_line_inside_block_closes() {
        let t = "```rust\na\n```toml\nb\n```\n";
        assert_eq!(scan(t), vec![fence("rust", 0, "a"), fence("", 4, "")]);
    }

    #[test]
    fn two_blocks() {
        let t = "```rust,no_run\none\n```\nmid\n```\ntwo\n```\n";
        assert_eq!(
            scan(t),
            vec![fence("rust,no_run", 0, "one"), fence("", 4, "two")]
        );
    }

    #[test]
    fn multiline_body_preserved() {
        let t = "```\nlet a = 1;\nlet b = 2;\n```\n";
        assert_eq!(scan(t), vec![fence("", 0, "let a = 1;\nlet b = 2;")]);
    }
}
