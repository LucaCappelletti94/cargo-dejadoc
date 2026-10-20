/// The fenced code blocks of a rustdoc comment stream, in source order.
pub(super) fn doc_blocks(text: &str) -> Vec<String> {
    let mut blocks: Vec<String> = Vec::new();
    let mut current: Option<String> = None;
    for raw in text.lines() {
        let line = raw.strip_prefix("//!").unwrap_or(raw);
        if line.trim_start().starts_with("```") {
            if let Some(block) = current.take() {
                blocks.push(block);
            } else {
                current = Some(String::new());
            }
        } else if let Some(block) = current.as_mut() {
            block.push_str(line);
            block.push('\n');
        }
    }
    blocks
}
