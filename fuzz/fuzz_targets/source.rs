//! A whole source file through `syn` and extraction.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(source) = std::str::from_utf8(data) else {
        return;
    };
    let source = source.to_owned();
    dejadoc_fuzz::on_large_stack(move || {
        if let Some(blocks) = dejadoc_fuzz::extract(&source) {
            dejadoc_fuzz::check_spans(&blocks);
        }
    });
});
