//! Harness pieces the dejadoc fuzz targets share.

use std::sync::LazyLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::time::Duration;

use dejadoc::DocTest;

type Job = Box<dyn FnOnce() + Send>;

/// Longest a target waits for the worker, past libFuzzer's own `-timeout`.
const REPLY_DEADLINE: Duration = Duration::from_secs(120);

/// The queue of one long-lived thread with the stack `dejadoc::group`
/// reserves, so a deep input reports a fault in dejadoc and not the end of
/// the fuzzer's stack, without a thread start per input.
static WORKER: LazyLock<Sender<Job>> = LazyLock::new(|| {
    let (jobs, queue) = mpsc::channel::<Job>();
    std::thread::Builder::new()
        .stack_size(1 << 30)
        .spawn(move || queue.into_iter().for_each(|job| job()))
        .expect("spawn the fuzz worker");
    jobs
});

/// Run `f` on the worker and return its value.
pub fn on_large_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let (reply, answer) = mpsc::sync_channel(1);
    WORKER
        .send(Box::new(move || {
            // The receiver only goes away after its deadline, the panic below reports that.
            let _ = reply.send((id, f()));
        }))
        .expect("the fuzz worker is running");
    let (answered, value) = answer
        .recv_timeout(REPLY_DEADLINE)
        .expect("the fuzz worker answers within its deadline");
    assert_eq!(answered, id, "reply to request {answered}, expected {id}");
    value
}

/// Extract the doctests of `source` as the root file of crate `c`.
pub fn extract(source: &str) -> Option<Vec<DocTest>> {
    let file = syn::parse_file(source).ok()?;
    Some(dejadoc::extract::extract(
        "c",
        "/r/src/lib.rs",
        &file,
        "/r",
        &|_, _| None,
    ))
}

/// Every reported block ends on or after its opening line.
pub fn check_spans(blocks: &[DocTest]) {
    for block in blocks {
        if let Some(end) = block.end {
            assert!(end >= block.line, "{block:?}");
        }
    }
}

/// The function report of `source` scanned as the modules `a` and `b` of one crate.
pub fn functions(source: &str) -> Option<dejadoc::Report> {
    let parsed = syn::parse_file(source).ok()?;
    let file = |segment: &str| dejadoc::SourceFile {
        path: format!("/r/src/{segment}.rs"),
        segments: vec![segment.to_string()],
        parsed: parsed.clone(),
        text: source.to_string(),
        rustdoc: false,
    };
    let target = dejadoc::TargetScan {
        name: "c".into(),
        files: vec![file("a"), file("b")],
        library: true,
    };
    Some(
        dejadoc::Dejadoc::default()
            .functions()
            .fn_min_tokens(0)
            .run_targets("/r", &[target], &|_, _| None),
    )
}

/// Every function group stays in one module, and both copies of the source group alike.
pub fn check_functions(report: &dejadoc::Report) {
    let mut per_file = [0usize; 2];
    for group in &report.groups {
        let file = &group.sites[0].file;
        for site in &group.sites {
            assert_eq!(&site.file, file, "{group:?}");
            assert!(site.end.is_some_and(|end| end >= site.line), "{site:?}");
        }
        per_file[usize::from(file.ends_with("b.rs"))] += 1;
    }
    assert_eq!(per_file[0], per_file[1], "{report:?}");
    assert_eq!(report.functions % 2, 0);
}
