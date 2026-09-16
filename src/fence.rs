//! Code block extraction from doc text through pulldown-cmark, the
//! parser rustdoc collects doctests with.
use alloc::string::String;
use alloc::vec::Vec;

use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};

/// A code block found in doc text: a fenced block (backticks or tildes)
/// or an indented block (four or more leading columns). Indented blocks
/// have an empty info string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CodeBlock {
    /// Info string after the opening fence markers, or empty for an
    /// indented block.
    pub(crate) info: String,
    /// 0-based index of the opening line within the doc text.
    pub(crate) line: usize,
    /// Body with container indentation removed, without the trailing
    /// newline.
    pub(crate) code: String,
}

/// rustdoc's markdown options for the main body of documentation.
const OPTIONS: Options = Options::ENABLE_TABLES
    .union(Options::ENABLE_FOOTNOTES)
    .union(Options::ENABLE_STRIKETHROUGH)
    .union(Options::ENABLE_TASKLISTS)
    .union(Options::ENABLE_SMART_PUNCTUATION);

/// Scan `text` for code blocks with rustdoc's markdown options. The
/// line is the opening fence or, for an indented block, its first line.
#[must_use]
pub(crate) fn scan(text: &str) -> Vec<CodeBlock> {
    let mut parser = Parser::new_ext(text, OPTIONS).into_offset_iter();
    let mut out = Vec::new();
    while let Some((event, range)) = parser.next() {
        let Event::Start(Tag::CodeBlock(kind)) = event else {
            continue;
        };
        let info = match kind {
            CodeBlockKind::Fenced(lang) => lang.into_string(),
            CodeBlockKind::Indented => String::new(),
        };
        let mut code = String::new();
        for (event, _) in parser.by_ref() {
            match event {
                Event::Text(s) => code.push_str(&s),
                Event::End(TagEnd::CodeBlock) => break,
                _ => {}
            }
        }
        if code.ends_with('\n') {
            code.pop();
        }
        out.push(CodeBlock {
            info,
            line: text[..range.start].matches('\n').count(),
            code,
        });
    }
    out
}
#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn block(info: &str, line: usize, code: &str) -> CodeBlock {
        CodeBlock {
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
        assert_eq!(scan(t), vec![block("rust", 1, "let x = 5;")]);
    }

    #[test]
    fn empty_info() {
        let t = "```\nfn main() {}\n```\n";
        assert_eq!(scan(t), vec![block("", 0, "fn main() {}")]);
    }

    #[test]
    fn four_space_fence_is_an_indented_block() {
        let t = "    ```rust\nx\n    ```\n";
        assert_eq!(scan(t), vec![block("", 0, "```rust")]);
    }

    #[test]
    fn tilde_fence_scans() {
        let t = "~~~rust\nx\n~~~\n";
        assert_eq!(scan(t), vec![block("rust", 0, "x")]);
    }

    #[test]
    fn footnote_continuation_is_not_a_block() {
        // Footnotes are on for rustdoc, so the indented line continues
        // the footnote instead of opening an indented block.
        let t = "[^1]: note\n\n    let x = 1;\n";
        assert_eq!(scan(t), vec![]);
    }
    #[test]
    fn nested_list_is_not_a_block() {
        let t = "- item\n\n    - nested a\n    - nested b\n\n  tail\n";
        assert_eq!(scan(t), vec![]);
    }

    #[test]
    fn fence_inside_list_item_dedents() {
        let t = "- item\n\n  ```\n  let a = 1;\n  ```\n\n- other\n\n    continuation\n\n    ```rust\n    let b = 2;\n    ```\n";
        assert_eq!(
            scan(t),
            vec![block("", 2, "let a = 1;"), block("rust", 10, "let b = 2;")]
        );
    }

    #[test]
    fn blockquote_fence_dedents() {
        let t = "> ```\n> let a = 1;\n> ```\n";
        assert_eq!(scan(t), vec![block("", 0, "let a = 1;")]);
    }

    #[test]
    fn backtick_line_inside_tilde_is_content() {
        let t = "~~~rust\nfn a() {}\n```\nfn b() {}\n~~~\n";
        assert_eq!(scan(t), vec![block("rust", 0, "fn a() {}\n```\nfn b() {}")]);
    }

    #[test]
    fn tilde_line_inside_backticks_is_content() {
        let t = "```rust\n~~~\nx\n```\n";
        assert_eq!(scan(t), vec![block("rust", 0, "~~~\nx")]);
    }

    #[test]
    fn two_tildes_do_not_open() {
        let t = "~~rust\nx\n~~\n```\ny\n```\n";
        assert_eq!(scan(t), vec![block("", 3, "y")]);
    }

    #[test]
    fn four_tildes_open_and_close() {
        let t = "~~~~rust\nx\n~~~~\n";
        assert_eq!(scan(t), vec![block("rust", 0, "x")]);
    }

    #[test]
    fn mixed_fences_both_scan() {
        let t = "~~~rust\nx\n~~~\n```\nfn main() {}\n```\n";
        assert_eq!(
            scan(t),
            vec![block("rust", 0, "x"), block("", 3, "fn main() {}")]
        );
    }

    #[test]
    fn unterminated_tilde_emits_at_eof() {
        let t = "~~~rust\nfn main() {}";
        assert_eq!(scan(t), vec![block("rust", 0, "fn main() {}")]);
    }

    #[test]
    fn unterminated_emits_at_eof() {
        let t = "```rust\nfn main() {}";
        assert_eq!(scan(t), vec![block("rust", 0, "fn main() {}")]);
    }

    #[test]
    fn closer_with_info_is_content() {
        let t = "```rust\na\n```toml\nb\n```\n";
        assert_eq!(scan(t), vec![block("rust", 0, "a\n```toml\nb")]);
    }

    #[test]
    fn two_blocks() {
        let t = "```rust,no_run\none\n```\nmid\n```\ntwo\n```\n";
        assert_eq!(
            scan(t),
            vec![block("rust,no_run", 0, "one"), block("", 4, "two")]
        );
    }

    #[test]
    fn multiline_body_preserved() {
        let t = "```\nlet a = 1;\nlet b = 2;\n```\n";
        assert_eq!(scan(t), vec![block("", 0, "let a = 1;\nlet b = 2;")]);
    }

    #[test]
    fn three_space_fence_opens() {
        let t = "   ```rust\nx\n```\n";
        assert_eq!(scan(t), vec![block("rust", 0, "x")]);
    }

    #[test]
    fn tab_indented_fence_is_an_indented_block() {
        let t = "\t```\nx\n```\n";
        assert_eq!(scan(t), vec![block("", 0, "```"), block("", 2, "")]);
    }

    #[test]
    fn closer_must_match_opener_length() {
        let t = "````\nx\n```\n````\n";
        assert_eq!(scan(t), vec![block("", 0, "x\n```")]);
    }

    #[test]
    fn longer_closer_closes() {
        let t = "```\nx\n````\n";
        assert_eq!(scan(t), vec![block("", 0, "x")]);
    }

    #[test]
    fn closer_trailing_whitespace_is_fine() {
        let t = "```\nx\n```   \n";
        assert_eq!(scan(t), vec![block("", 0, "x")]);
    }

    #[test]
    fn three_space_closer_closes() {
        let t = "```rust\nfn main() {}\n   ```\nfn other() {}\n";
        assert_eq!(scan(t), vec![block("rust", 0, "fn main() {}")]);
    }

    #[test]
    fn backtick_in_backtick_info_is_not_a_fence() {
        let t = "``` `x`\ny\n```\n";
        assert_eq!(scan(t), vec![block("", 2, "")]);
    }

    #[test]
    fn tilde_info_may_contain_backticks() {
        let t = "~~~ `x`\ny\n~~~\n";
        assert_eq!(scan(t), vec![block("`x`", 0, "y")]);
    }

    #[test]
    fn indented_block_after_blank_line() {
        let t = "intro\n\n    let x = 1;\n    let y = 2;\n";
        assert_eq!(scan(t), vec![block("", 2, "let x = 1;\nlet y = 2;")]);
    }

    #[test]
    fn indented_block_at_doc_start() {
        let t = "    let x = 1;\n";
        assert_eq!(scan(t), vec![block("", 0, "let x = 1;")]);
    }

    #[test]
    fn indented_line_after_text_is_lazy() {
        let t = "intro\n    let x = 1;\n";
        assert_eq!(scan(t), vec![]);
    }

    #[test]
    fn indented_block_internal_blank_kept() {
        let t = "    a\n\n    b\n";
        assert_eq!(scan(t), vec![block("", 0, "a\n\nb")]);
    }

    #[test]
    fn indented_block_trailing_blank_dropped() {
        let t = "    a\n\n";
        assert_eq!(scan(t), vec![block("", 0, "a")]);
    }

    #[test]
    fn indented_block_tabs_dedented() {
        let t = "\ta\n\tb\n";
        assert_eq!(scan(t), vec![block("", 0, "a\nb")]);
    }

    #[test]
    fn indented_block_three_spaces_plus_tab() {
        let t = "   \t c\n";
        assert_eq!(scan(t), vec![block("", 0, " c")]);
    }

    #[test]
    fn indented_block_dedents_four_columns() {
        let t = "        a\n    b\n";
        assert_eq!(scan(t), vec![block("", 0, "    a\nb")]);
    }

    #[test]
    fn indented_block_ends_at_fence() {
        let t = "    a\n```\nx\n```\n";
        assert_eq!(scan(t), vec![block("", 0, "a"), block("", 1, "x")]);
    }

    #[test]
    fn indented_block_after_fence_closer() {
        let t = "```\nx\n```\n    a\n";
        assert_eq!(scan(t), vec![block("", 0, "x"), block("", 3, "a")]);
    }
}
