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

/// Legal typed schedules group together while a changed operand remains distinct.
pub fn check_dependency_schedules(data: &[u8]) {
    let byte = |index| u32::from(data.get(index).copied().unwrap_or(0));
    let mask = byte(0);
    let shift = byte(1) % 32;
    let bias = byte(2);
    let (first, second) = if mask & 1 == 0 {
        ("first", "second")
    } else {
        ("r#type", "r#loop")
    };
    let statements = [
        format!("let {first} = input & {mask}u32;"),
        format!("let {second} = input >> {shift}u32;"),
        format!("let left = {first} ^ {bias}u32;"),
        format!("let right = {second} | 1u32;"),
    ];
    let source = |order: [usize; 4]| {
        format!(
            "fn f(input:u32)->(u32,u32){{ {} {} {} {} (left,right) }}",
            statements[order[0]], statements[order[1]], statements[order[2]], statements[order[3]],
        )
    };
    let original = source([0, 1, 2, 3]);
    let scheduled = source([1, 3, 0, 2]);
    let changed = original.replace(&format!("^ {bias}u32"), &format!("^ {}u32", bias + 1));
    let site = |item: &str, code: String| DocTest {
        file: format!("src/{item}.rs"),
        line: 1,
        end: None,
        item: format!("c::{item}"),
        info: Vec::new(),
        code,
        allow: false,
        self_type: None,
        public: false,
    };
    let report = dejadoc::group(
        &[
            site("original", original),
            site("scheduled", scheduled),
            site("changed", changed),
        ],
        2,
        0,
    );
    let groups: Vec<_> = report
        .groups
        .iter()
        .map(|group| {
            let mut members: Vec<_> = group.sites.iter().map(|site| site.item.as_str()).collect();
            members.sort_unstable();
            members
        })
        .collect();
    assert_eq!(groups, [vec!["c::original", "c::scheduled"]]);
}
