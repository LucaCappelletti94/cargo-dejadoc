//! Doc text through extraction. NUL bytes split the input into parts, each
//! written in turn as `///` lines, a raw literal, an escaped literal and a
//! `/** */` block, so the parts merge into one doc stream the way rustdoc's do.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let mut source = String::new();
    for (index, part) in text.split('\0').enumerate() {
        match index % 4 {
            0 if !part.contains('\r') => {
                for line in part.split('\n') {
                    source.push_str(&format!("///{line}\n"));
                }
            }
            1 if !part.contains("\"#") => source.push_str(&format!("#[doc = r#\"{part}\"#]\n")),
            3 if !part.contains("*/") => source.push_str(&format!("/**{part}*/\n")),
            _ => source.push_str(&format!("#[doc = {part:?}]\n")),
        }
    }
    source.push_str("pub fn f() {}\n");
    dejadoc_fuzz::on_large_stack(move || {
        if let Some(blocks) = dejadoc_fuzz::extract(&source) {
            dejadoc_fuzz::check_spans(&blocks);
        }
    });
});
