//! Code block extraction from doc text, mirroring rustdoc's
//! pulldown-cmark doctest collection.
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec;
use alloc::vec::Vec;

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
    /// Body lines joined by `'\n'`, indented blocks dedented by four
    /// columns.
    pub(crate) code: String,
}

/// The class of the line before the current one, for the indented
/// block rule that a block cannot interrupt a text line.
#[derive(PartialEq)]
enum Prev {
    Start,
    Blank,
    Text,
    BlockEnd,
}

/// Scan `text` for code blocks, matching rustdoc. A fence of at least
/// three backticks or tildes with at most three leading spaces opens a
/// fenced block; its closer is the same character, at least as long, at
/// most three leading spaces, with nothing but trailing spaces or tabs.
/// A backtick in a backtick fence's info string disqualifies the fence.
/// A line of four or more leading columns (a tab counts to the next
/// multiple of four) starts an indented block unless the previous line
/// is text; its body drops four columns per line. Unterminated blocks
/// emit at EOF.
#[must_use]
pub(crate) fn scan(text: &str) -> Vec<CodeBlock> {
    let lines: Vec<&str> = text.lines().collect();
    let total = lines.len();
    let mut out: Vec<CodeBlock> = Vec::new();
    scan_lines(&lines, total, Prev::Start, &mut out);
    out
}

/// Classify the lines one at a time until the slice is empty.
fn scan_lines<'a>(
    mut lines: &'a [&'a str],
    total: usize,
    mut prev: Prev,
    out: &mut Vec<CodeBlock>,
) {
    while let [first, rest @ ..] = lines {
        let line = total - lines.len();
        if first.trim().is_empty() {
            prev = Prev::Blank;
            lines = rest;
            continue;
        }
        if let Some((c, len)) = fence_open(first) {
            lines = fenced_block(first, rest, c, len, line, out);
            prev = Prev::BlockEnd;
            continue;
        }
        if indent_cols(first) >= 4 && matches!(prev, Prev::Start | Prev::Blank | Prev::BlockEnd) {
            lines = indented_block(first, rest, line, out);
            prev = Prev::BlockEnd;
            continue;
        }
        prev = Prev::Text;
        lines = rest;
    }
}

/// Consume one fenced block opened by `opener`; return the remaining
/// lines.
fn fenced_block<'a>(
    opener: &str,
    rest: &'a [&'a str],
    c: char,
    len: usize,
    line: usize,
    out: &mut Vec<CodeBlock>,
) -> &'a [&'a str] {
    let info = fence_info(opener, c);
    let mut body: Vec<String> = Vec::new();
    let mut lines = rest;
    while let [first, rest2 @ ..] = lines {
        if fence_close(first, c, len) {
            lines = rest2;
            break;
        }
        body.push(first.to_string());
        lines = rest2;
    }
    out.push(CodeBlock {
        info,
        line,
        code: body.join("\n"),
    });
    lines
}

/// Consume one indented block whose first content line is `opener`;
/// return the remaining lines.
fn indented_block<'a>(
    opener: &str,
    rest: &'a [&'a str],
    line: usize,
    out: &mut Vec<CodeBlock>,
) -> &'a [&'a str] {
    let mut body: Vec<String> = vec![dedent4(opener).to_string()];
    let mut pending_blank = false;
    let mut lines = rest;
    while let [first, rest2 @ ..] = lines {
        if first.trim().is_empty() {
            if pending_blank {
                body.push(String::new());
            }
            pending_blank = true;
            lines = rest2;
            continue;
        }
        if indent_cols(first) < 4 {
            break;
        }
        if pending_blank {
            body.push(String::new());
            pending_blank = false;
        }
        body.push(dedent4(first).to_string());
        lines = rest2;
    }
    out.push(CodeBlock {
        info: String::new(),
        line,
        code: body.join("\n"),
    });
    lines
}

/// An opening fence: the character and marker length, when the line is
/// a valid opener.
fn fence_open(line: &str) -> Option<(char, usize)> {
    let indent = line.bytes().take_while(|&b| b == b' ').count();
    if indent > 3 {
        return None;
    }
    let rest = &line[indent..];
    let c = rest.chars().next()?;
    if c != '`' && c != '~' {
        return None;
    }
    let len = rest.bytes().take_while(|&b| b as char == c).count();
    if len < 3 {
        return None;
    }
    if c == '`' && fence_info(line, c).contains('`') {
        return None;
    }
    Some((c, len))
}

/// The info string of an opening fence line: the trimmed rest after the
/// marker.
fn fence_info(line: &str, c: char) -> String {
    let indent = line.bytes().take_while(|&b| b == b' ').count();
    let rest = &line[indent..];
    let len = rest.bytes().take_while(|&b| b as char == c).count();
    rest[len..].trim().to_string()
}

/// A closing fence: same character, at least the opener's length, at
/// most three leading spaces, nothing but trailing spaces or tabs.
fn fence_close(line: &str, c: char, len: usize) -> bool {
    let indent = line.bytes().take_while(|&b| b == b' ').count();
    if indent > 3 {
        return false;
    }
    let rest = &line[indent..];
    let n = rest.bytes().take_while(|&b| b as char == c).count();
    n >= len && rest[n..].chars().all(|ch| ch == ' ' || ch == '\t')
}

/// Leading whitespace columns of a line; a tab advances to the next
/// multiple of four.
fn indent_cols(line: &str) -> usize {
    let mut col = 0usize;
    for ch in line.chars() {
        if ch == ' ' {
            col += 1;
        } else if ch == '\t' {
            col += 4 - (col % 4);
        } else {
            break;
        }
    }
    col
}

/// The line without its first four columns.
fn dedent4(line: &str) -> &str {
    let mut col = 0usize;
    for (i, ch) in line.char_indices() {
        if col >= 4 {
            return &line[i..];
        }
        col += if ch == '\t' { 4 - (col % 4) } else { 1 };
    }
    line
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
