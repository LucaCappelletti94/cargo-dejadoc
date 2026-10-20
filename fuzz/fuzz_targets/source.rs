//! A whole source file through `syn`, extraction and the function check.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(source) = std::str::from_utf8(data) else {
        return;
    };
    let source = source.to_owned();
    echoes_fuzz::on_large_stack(move || {
        if let Some(blocks) = echoes_fuzz::extract(&source) {
            echoes_fuzz::check_spans(&blocks);
        }
        if let Some(report) = echoes_fuzz::functions(&source) {
            echoes_fuzz::check_functions(&report);
        }
    });
});
