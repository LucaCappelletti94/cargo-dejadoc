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

/// Checked literal folds and legal algebra and schedules group apart from changed neighbors.
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
        format!("let {first} = input & ({mask}u32 + {bias}u32);"),
        format!("let {second} = input >> ({shift}u32 + 0u32);"),
        format!("let left = ({first} ^ {bias}u32) ^ 7u32;"),
        format!("let right = {second} | 1u32;"),
        format!("let flag = {first} > 3u32;"),
    ];
    let fn_code = |p0: &str, p1: &str, p2: &str, p3: &str, p4: &str, output: &str| {
        format!("fn f(input:u32)->(u32,u32,bool){{ {p0} {p1} {p2} {p3} {p4} {output} }}")
    };
    let original = fn_code(
        &statements[0],
        &statements[1],
        &statements[2],
        &statements[3],
        &statements[4],
        "(left,right,flag)",
    );
    let scheduled = {
        let s0 = format!("let {second} = input >> {shift}u32;");
        let s1 = format!("let {first} = input & {}u32;", mask + bias);
        fn_code(
            &s0,
            &s1,
            &statements[4],
            &statements[3],
            &statements[2],
            "(left,right,flag)",
        )
    };
    let commute = {
        let s2 = format!("let left = 7u32 ^ ({first} ^ {bias}u32);");
        fn_code(
            &statements[0],
            &statements[1],
            &s2,
            &statements[3],
            &statements[4],
            "(left,right,flag)",
        )
    };
    let associate = {
        let s2 = format!("let left = {first} ^ ({bias}u32 ^ 7u32);");
        fn_code(
            &statements[0],
            &statements[1],
            &s2,
            &statements[3],
            &statements[4],
            "(left,right,flag)",
        )
    };
    let idempotent = {
        let s3 = format!("let right = ({second} | 1u32) | ({second} | 1u32);");
        fn_code(
            &statements[0],
            &statements[1],
            &statements[2],
            &s3,
            &statements[4],
            "(left,right,flag)",
        )
    };
    let compare = {
        let s4 = format!("let flag = 3u32 < {first};");
        fn_code(
            &statements[0],
            &statements[1],
            &statements[2],
            &statements[3],
            &s4,
            "(left,right,flag)",
        )
    };
    let changed = {
        let s2 = format!("let left = ({first} ^ {}u32) ^ 7u32;", bias + 1);
        fn_code(
            &statements[0],
            &statements[1],
            &s2,
            &statements[3],
            &statements[4],
            "(left,right,flag)",
        )
    };
    let operator = {
        let s2 = format!("let left = ({first} | {bias}u32) ^ 7u32;");
        fn_code(
            &statements[0],
            &statements[1],
            &s2,
            &statements[3],
            &statements[4],
            "(left,right,flag)",
        )
    };
    let port = fn_code(
        &statements[0],
        &statements[1],
        &statements[2],
        &statements[3],
        &statements[4],
        "(right,left,flag)",
    );
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
            site("commute", commute),
            site("associate", associate),
            site("idempotent", idempotent),
            site("compare", compare),
            site("changed", changed),
            site("operator", operator),
            site("port", port),
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
    assert_eq!(
        groups,
        [vec![
            "c::associate",
            "c::commute",
            "c::compare",
            "c::idempotent",
            "c::original",
            "c::scheduled",
        ]]
    );
}

/// The six boolean families fold apart from changed truth-value, operand-origin and root-order neighbors.
pub fn check_boolean_schedules(data: &[u8]) {
    let byte = |index| u32::from(data.get(index).copied().unwrap_or(0));
    let flags = byte(0);
    let lo = byte(1) % 128;
    let hi = (lo + 1 + byte(2) % 128) % 256;
    let (tail_true, tail_false) = if flags & 1 == 0 {
        ("true", "false")
    } else {
        ("false", "true")
    };
    let folded_tail = if flags & 1 == 0 { "a" } else { "!a" };
    let (join, dual) = if flags & 2 == 0 {
        ("&&", "||")
    } else {
        ("||", "&&")
    };
    let (literal, selected) = if flags & 4 == 0 {
        ("true", lo)
    } else {
        ("false", hi)
    };
    let (parity_cond, v16_first, v16_second) = if flags & 8 == 0 {
        ("!b", hi, lo)
    } else {
        ("!!b", lo, hi)
    };
    let statements = [
        "let n04 = !!a;".to_owned(),
        format!("let n05 = if a {{ {tail_true} }} else {{ {tail_false} }};"),
        format!("let n06 = !(a {join} b);"),
        "let n11 = b == true;".to_owned(),
        format!("let n16 = if {parity_cond} {{ {lo}u8 }} else {{ {hi}u8 }};"),
        format!("let n19 = if {literal} {{ {lo}u8 }} else {{ {hi}u8 }};"),
    ];
    let fn_code = |n04: &str,
                   n05: &str,
                   n06: &str,
                   n11: &str,
                   n16: &str,
                   n19: &str,
                   output: &str| {
        format!(
            "fn f(a:bool,b:bool)->(bool,bool,bool,bool,u8,u8){{ {n04} {n05} {n06} {n11} {n16} {n19} {output} }}"
        )
    };
    let roots = "(n04,n05,n06,n11,n16,n19)";
    let original = fn_code(
        &statements[0],
        &statements[1],
        &statements[2],
        &statements[3],
        &statements[4],
        &statements[5],
        roots,
    );
    let v04 = fn_code(
        "let n04 = a;",
        &statements[1],
        &statements[2],
        &statements[3],
        &statements[4],
        &statements[5],
        roots,
    );
    let v05 = fn_code(
        &statements[0],
        &format!("let n05 = {folded_tail};"),
        &statements[2],
        &statements[3],
        &statements[4],
        &statements[5],
        roots,
    );
    let v06 = fn_code(
        &statements[0],
        &statements[1],
        &format!("let n06 = !a {dual} !b;"),
        &statements[3],
        &statements[4],
        &statements[5],
        roots,
    );
    let v11 = fn_code(
        &statements[0],
        &statements[1],
        &statements[2],
        "let n11 = b;",
        &statements[4],
        &statements[5],
        roots,
    );
    let v16 = fn_code(
        &statements[0],
        &statements[1],
        &statements[2],
        &statements[3],
        &format!("let n16 = if b {{ {v16_first}u8 }} else {{ {v16_second}u8 }};"),
        &statements[5],
        roots,
    );
    let v19 = fn_code(
        &statements[0],
        &statements[1],
        &statements[2],
        &statements[3],
        &statements[4],
        &format!("let n19 = {selected}u8;"),
        roots,
    );
    let truth = fn_code(
        &statements[0],
        &format!("let n05 = if a {{ {tail_false} }} else {{ {tail_true} }};"),
        &statements[2],
        &statements[3],
        &statements[4],
        &statements[5],
        roots,
    );
    let origin = fn_code(
        &statements[0],
        &statements[1],
        &format!("let n06 = !(a {join} a);"),
        &statements[3],
        &statements[4],
        &statements[5],
        roots,
    );
    let port = fn_code(
        &statements[0],
        &statements[1],
        &statements[2],
        &statements[3],
        &statements[4],
        &statements[5],
        "(n11,n05,n06,n04,n16,n19)",
    );
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
            site("v04", v04),
            site("v05", v05),
            site("v06", v06),
            site("v11", v11),
            site("v16", v16),
            site("v19", v19),
            site("truth", truth),
            site("origin", origin),
            site("port", port),
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
    assert_eq!(
        groups,
        [vec![
            "c::original",
            "c::v04",
            "c::v05",
            "c::v06",
            "c::v11",
            "c::v16",
            "c::v19",
        ]]
    );
}
