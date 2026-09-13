//! Fenced code block extraction from doc text.
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;

/// A fenced code block found in doc text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Fence {
    /// Info string between the fence markers and the line end.
    pub(crate) info: String,
    /// 0-based index of the opening fence line within the doc text.
    pub(crate) line: usize,
    /// Body lines joined by `'\n'`.
    pub(crate) code: String,
}

/// Scan `text` for fenced code blocks. A backtick or tilde fence of at
/// least three characters opens a block; a block closes only on the same
/// character. An unterminated block at EOF is emitted, matching rustdoc.
#[must_use]
pub(crate) fn scan(text: &str) -> Vec<Fence> {
    let mut out = Vec::new();
    let mut open: Option<(char, String, usize, Vec<String>)> = None;
    for (i, line) in text.lines().enumerate() {
        let mark = fence_char(line);
        match (open.take(), mark) {
            (None, Some(c)) => {
                open = Some((c, info_of(line, c), i, Vec::new()));
            }
            (Some(f), Some(c)) if c == f.0 => out.push(Fence {
                info: f.1,
                line: f.2,
                code: f.3.join("\n"),
            }),
            (Some(mut f), _) => {
                f.3.push(line.to_string());
                open = Some(f);
            }
            (None, None) => {}
        }
    }
    if let Some((_, info, line, body)) = open {
        out.push(Fence {
            info,
            line,
            code: body.join("\n"),
        });
    }
    out
}

/// The fence character of a line when the line starts with three or more
/// of it.
fn fence_char(line: &str) -> Option<char> {
    let t = line.trim_start();
    if t.starts_with("```") {
        Some('`')
    } else if t.starts_with("~~~") {
        Some('~')
    } else {
        None
    }
}

/// The info string of an opening fence line.
fn info_of(line: &str, c: char) -> String {
    line.trim_start().trim_start_matches(c).trim().to_string()
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
    fn tilde_fence_scans() {
        let t = "~~~rust\nx\n~~~\n";
        assert_eq!(scan(t), vec![fence("rust", 0, "x")]);
    }

    #[test]
    fn backtick_line_inside_tilde_is_content() {
        let t = "~~~rust\nfn a() {}\n```\nfn b() {}\n~~~\n";
        assert_eq!(scan(t), vec![fence("rust", 0, "fn a() {}\n```\nfn b() {}")]);
    }

    #[test]
    fn tilde_line_inside_backticks_is_content() {
        let t = "```rust\n~~~\nx\n```\n";
        assert_eq!(scan(t), vec![fence("rust", 0, "~~~\nx")]);
    }

    #[test]
    fn two_tildes_do_not_open() {
        let t = "~~rust\nx\n~~\n```\ny\n```\n";
        assert_eq!(scan(t), vec![fence("", 3, "y")]);
    }

    #[test]
    fn four_tildes_open_and_close() {
        let t = "~~~~rust\nx\n~~~~\n";
        assert_eq!(scan(t), vec![fence("rust", 0, "x")]);
    }

    #[test]
    fn mixed_fences_both_scan() {
        let t = "~~~rust\nx\n~~~\n```\nfn main() {}\n```\n";
        assert_eq!(
            scan(t),
            vec![fence("rust", 0, "x"), fence("", 3, "fn main() {}")]
        );
    }

    #[test]
    fn unterminated_tilde_emits_at_eof() {
        let t = "~~~rust\nfn main() {}";
        assert_eq!(scan(t), vec![fence("rust", 0, "fn main() {}")]);
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
