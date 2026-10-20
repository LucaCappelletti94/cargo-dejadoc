//! Harness pieces the echoes fuzz targets share.

use std::sync::LazyLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::time::Duration;

use echoes::DocTest;

type Job = Box<dyn FnOnce() + Send>;

/// Longest a target waits for the worker, past libFuzzer's own `-timeout`.
const REPLY_DEADLINE: Duration = Duration::from_secs(120);

/// The queue of one long-lived thread with the stack `echoes::group`
/// reserves, so a deep input reports a fault in echoes and not the end of
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
    Some(echoes::extract::extract(
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
pub fn functions(source: &str) -> Option<echoes::Report> {
    let parsed = syn::parse_file(source).ok()?;
    let file = |segment: &str| echoes::SourceFile {
        path: format!("/r/src/{segment}.rs"),
        segments: vec![segment.to_string()],
        cfg: Vec::new(),
        parsed: parsed.clone(),
        text: source.to_string(),
        rustdoc: false,
    };
    let target = echoes::TargetScan {
        name: "c".into(),
        files: vec![file("a"), file("b")],
        library: true,
    };
    Some(
        echoes::Echoes::default()
            .functions()
            .fn_min_tokens(0)
            .run_targets("/r", &[target], &|_, _| None),
    )
}

/// Every function group stays in one module, and both copies of the source group alike.
pub fn check_functions(report: &echoes::Report) {
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
    let report = echoes::group(
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
    let report = echoes::group(
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

/// Primitive value folds keep exact membership and distinct value, storage and evaluation neighbors.
pub fn check_value_schedules(data: &[u8]) {
    let byte = |index| u32::from(data.get(index).copied().unwrap_or(0));
    let mask = byte(0);
    let lo = byte(1) % 128;
    let bias = 1 + (byte(1) + byte(2)) % 255;
    let shift = byte(2) % 32;
    let dead = (byte(0) + byte(1) + byte(2) + 1) % 256;
    let mask_stmt = format!("let mask=input&{mask}u32;");
    let alias_stmt = "let copied=mask;";
    let temp_stmt = format!("let temp=copied^{lo}u32;");
    let tuple_stmt = format!("let (tx,ty)=(mask|{bias}u32,mask>>{shift}u32);");
    let dead_stmt = format!("let dead={dead}u32;");
    let flag_stmt = "let flag=tx>3u32;";
    let n_stmt = "let n=if flag{true}else{false};";
    let s: [&str; 7] = [
        &mask_stmt,
        alias_stmt,
        &temp_stmt,
        &tuple_stmt,
        &dead_stmt,
        flag_stmt,
        n_stmt,
    ];
    let tx_stmt = format!("let tx=mask|{bias}u32;");
    let ty_stmt = format!("let ty=mask>>{shift}u32;");
    let fn_code = |parts: &[&str], output: &str| {
        let prefix = "fn f(input:u32)->(u32,bool,u32){";
        let capacity =
            prefix.len() + parts.iter().map(|part| part.len()).sum::<usize>() + output.len() + 1;
        let mut code = String::with_capacity(capacity);
        code.push_str(prefix);
        for part in parts {
            code.push_str(part);
        }
        code.push_str(output);
        code.push('}');
        code
    };
    let original = fn_code(&[s[0], s[1], s[2], s[3], s[4], s[5], s[6]], "(temp,n,ty)");
    let v07_temp = format!("let temp=mask^{lo}u32;");
    let v07 = fn_code(&[s[0], &v07_temp, s[3], s[4], s[5], s[6]], "(temp,n,ty)");
    let v08 = fn_code(
        &[s[0], s[1], s[3], s[4], s[5], s[6]],
        &format!("(copied^{lo}u32,n,ty)"),
    );
    let v10 = fn_code(
        &[
            s[0],
            s[1],
            s[2],
            tx_stmt.as_str(),
            ty_stmt.as_str(),
            s[4],
            s[5],
            s[6],
        ],
        "(temp,n,ty)",
    );
    let v18 = fn_code(&[s[0], s[1], s[2], s[3], s[5], s[6]], "(temp,n,ty)");
    let combined = fn_code(
        &[s[0], tx_stmt.as_str(), ty_stmt.as_str(), s[5], s[6]],
        &format!("(mask^{lo}u32,n,ty)"),
    );
    let truth_stmt = "let n=if flag{false}else{true};";
    let truth = fn_code(
        &[s[0], s[1], s[2], s[3], s[4], s[5], truth_stmt],
        "(temp,n,ty)",
    );
    let origin = fn_code(&[s[0], s[1], s[2], s[3], s[4], s[5], s[6]], "(temp,n,tx)");
    let storage = fn_code(
        &[s[0], s[1], "let _=&copied;", s[2], s[3], s[4], s[5], s[6]],
        "(temp,n,ty)",
    );
    let evaluation_stmt = format!("let temp=copied|{}u32;", lo + 1);
    let evaluation = fn_code(
        &[s[0], s[1], evaluation_stmt.as_str(), s[3], s[4], s[5], s[6]],
        "(temp,n,ty)",
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
    let report = echoes::group(
        &[
            site("original", original),
            site("v07", v07),
            site("v08", v08),
            site("v10", v10),
            site("v18", v18),
            site("combined", combined),
            site("truth", truth),
            site("origin", origin),
            site("storage", storage),
            site("evaluation", evaluation),
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
            "c::combined",
            "c::original",
            "c::v07",
            "c::v08",
            "c::v10",
            "c::v18",
        ]]
    );
}

/// Pure helper schedules preserve origins, effects, shadowing and argument order.
pub fn check_helper_schedules(data: &[u8]) {
    let byte = |index| u32::from(data.get(index).copied().unwrap_or(0));
    let mask = byte(0);
    let shift = byte(1) % 32;
    let bias = byte(3);
    let attr = if byte(2) & 2 == 0 {
        "#[track_caller]"
    } else {
        "#[cfg(all())]"
    };
    let (h1, h2, low, b2, value) = if byte(2) & 1 == 0 {
        ("one", "two", "low", "two", "x")
    } else {
        ("r#type", "r#loop", "r#const", "r#match", "r#value")
    };
    let a_helpers = |h1: &str, h2: &str, mask: u32| {
        format!(
            "fn {h1}({value}:u32,q:bool)->u32{{let y={value}&{mask}u32;if q{{y}}else{{{value}>>{shift}u32}}}}fn {h2}({value}:u32)->u32{{{value}>>{shift}u32}}"
        )
    };
    let b_helpers = format!(
        "fn {low}({value}:u32)->u32{{{value}&{mask}u32}}fn {h1}({value}:u32)->u32{{{low}({value})}}fn {b2}({value}:u32)->u32{{{low}({value})}}"
    );
    let a_call = format!("let a={h1}(input,flag);");
    let b_call = format!("let b={h2}(input);");
    let a_h2 = format!("let a={h2}(input);");
    let b_h1 = format!("let b={h1}(input,flag);");
    let t_a_h1 = format!("let a={h1}(input);");
    let t_b_b2 = format!("let b={b2}(input);");
    let t_a_b2 = format!("let a={b2}(input);");
    let t_b_h1 = format!("let b={h1}(input);");
    let f_a = |body: &str| format!("fn f(input:u32,flag:bool)->(u32,u32){{{body}}}");
    let f_b = |body: &str| format!("fn f(input:u32)->(u32,u32){{{body}}}");
    let a_original = f_a(&format!("{a_call}{b_call}(a,b)"));
    let a_scheduled = f_a(&format!("{b_call}{a_call}(a,b)"));
    let a_renamed = f_a("let a=lo(input,flag);let b=hi(input);(a,b)");
    let n_origin = f_a(&format!("{a_h2}{b_h1}(a,b)"));
    let n_value = f_a(&format!("{a_call}{b_call}(a,b)"));
    let n_port = f_a(&format!("{a_call}{b_call}(b,a)"));
    let b_original = f_b(&format!("{t_a_h1}{t_b_b2}(a,b)"));
    let b_scheduled = f_b(&format!("{t_b_b2}{t_a_h1}(a,b)"));
    let n_origin_b = f_b(&format!("{t_a_b2}{t_b_h1}(a,b)"));
    let pair_a = format!("let a=helper(x);let b=helper(x^{bias}u32);");
    let pair_b = format!("let b=helper(x^{bias}u32);let a=helper(x);");
    let f_pair = |helper: &str, lead: &str, statements: &str| {
        format!("{helper}fn f(x:u32)->(u32,u32){{{lead}{statements}(a,b)}}")
    };
    let lead_shadow = "let helper=|v:u32|{println!(\"{v}\");v};";
    let n_shadow_a = f_pair(
        &format!("fn helper(x:u32)->u32{{x&{mask}u32}}"),
        lead_shadow,
        &pair_a,
    );
    let n_shadow_b = f_pair(
        &format!("fn helper(x:u32)->u32{{x&{mask}u32}}"),
        lead_shadow,
        &pair_b,
    );
    let n_recursive_a = f_pair("fn helper(x:u32)->u32{helper(x)}", "", &pair_a);
    let n_recursive_b = f_pair("fn helper(x:u32)->u32{helper(x)}", "", &pair_b);
    let n_diverge_a = f_pair("fn helper(x:u32)->u32{loop{}}", "", &pair_a);
    let n_diverge_b = f_pair("fn helper(x:u32)->u32{loop{}}", "", &pair_b);
    let n_observer_a = f_pair(
        &format!("{attr}fn helper(x:u32)->u32{{x&{mask}u32}}"),
        "",
        &pair_a,
    );
    let n_observer_b = f_pair(
        &format!("{attr}fn helper(x:u32)->u32{{x&{mask}u32}}"),
        "",
        &pair_b,
    );
    let helper_effect = "fn effect(x:u32)->u32{println!(\"{x}\");x}";
    let impure_a = format!("let a={h1}(effect(input),flag);let b={h2}(input);(a,b)");
    let impure_b = format!("let b={h2}(input);let a={h1}(effect(input),flag);(a,b)");
    let n_impure_a = f_a(&impure_a);
    let n_impure_b = f_a(&impure_b);
    let total_a = format!("let a={h1}(loop{{}},flag);let b={h2}(input);(a,b)");
    let total_b = format!("let b={h2}(input);let a={h1}(loop{{}},flag);(a,b)");
    let n_total_a = f_a(&total_a);
    let n_total_b = f_a(&total_b);
    let a_code = a_helpers(h1, h2, mask);
    let a_code_renamed = a_helpers("lo", "hi", mask);
    let a_code_value = a_helpers(h1, h2, mask.wrapping_add(1));
    let main_a = "fn main(){let _=f(0u32,false);}";
    let main_b = "fn main(){let _=f(0u32);}";
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
    let report = echoes::group(
        &[
            site("a_original", format!("{a_code}{a_original}{main_a}")),
            site("a_scheduled", format!("{a_code}{a_scheduled}{main_a}")),
            site("a_renamed", format!("{a_code_renamed}{a_renamed}{main_a}")),
            site("b_original", format!("{b_helpers}{b_original}{main_b}")),
            site("b_scheduled", format!("{b_helpers}{b_scheduled}{main_b}")),
            site("n_origin", format!("{a_code}{n_origin}{main_a}")),
            site("n_origin_b", format!("{b_helpers}{n_origin_b}{main_b}")),
            site("n_value", format!("{a_code_value}{n_value}{main_a}")),
            site("n_port", format!("{a_code}{n_port}{main_a}")),
            site(
                "n_impure_a",
                format!("{a_code}{helper_effect}{n_impure_a}{main_a}"),
            ),
            site(
                "n_impure_b",
                format!("{a_code}{helper_effect}{n_impure_b}{main_a}"),
            ),
            site("n_shadow_a", format!("{n_shadow_a}{main_b}")),
            site("n_shadow_b", format!("{n_shadow_b}{main_b}")),
            site("n_recursive_a", format!("{n_recursive_a}{main_b}")),
            site("n_recursive_b", format!("{n_recursive_b}{main_b}")),
            site("n_diverge_a", format!("{n_diverge_a}{main_b}")),
            site("n_diverge_b", format!("{n_diverge_b}{main_b}")),
            site("n_observer_a", format!("{n_observer_a}{main_b}")),
            site("n_observer_b", format!("{n_observer_b}{main_b}")),
            site("n_total_a", format!("{a_code}{n_total_a}{main_a}")),
            site("n_total_b", format!("{a_code}{n_total_b}{main_a}")),
        ],
        2,
        0,
    );
    let mut groups: Vec<Vec<&str>> = report
        .groups
        .iter()
        .map(|group| {
            let mut members: Vec<_> = group.sites.iter().map(|site| site.item.as_str()).collect();
            members.sort_unstable();
            members
        })
        .collect();
    groups.sort_unstable();
    assert_eq!(
        groups,
        [
            vec!["c::a_original", "c::a_renamed", "c::a_scheduled"],
            vec!["c::b_original", "c::b_scheduled", "c::n_origin_b"],
            vec!["c::n_origin", "c::n_port"],
        ]
    );
}

/// Named field initializers and literal alternatives keep labels, effects and multiplicity.
pub fn check_structure_schedules(data: &[u8]) {
    let byte = |index| u32::from(data.get(index).copied().unwrap_or(0));
    let mask = byte(0);
    let shift = byte(1) % 26 + 4;
    let low = 1 + byte(3) % 3;
    let (fa, fb) = if mask & 1 == 0 {
        ("a", "b")
    } else {
        ("r#type", "r#const")
    };
    let decl_p = format!("struct P{{{fa}:u32,{fb}:u32}}");
    let init_a = format!("input&{mask}u32");
    let init_b = format!("input>>{shift}u32");
    let tail = format!("(s.{fa},s.{fb})");
    let p_body =
        |fields: &str| format!("{decl_p}fn f(input:u32)->(u32,u32){{let s=P{{{fields}}};{tail}}}");
    let p_alias = |fields: &str| {
        format!("{decl_p}fn f(input:u32)->(u32,u32){{let low={init_b};let s=P{{{fields}}};{tail}}}")
    };
    let p_helper = |fields: &str| {
        format!(
            "fn h(u:u32)->u32{{u&{mask}u32}}{decl_p}fn f(input:u32)->(u32,u32){{let s=P{{{fields}}};{tail}}}"
        )
    };
    let p_base = |fields: &str| {
        format!(
            "{decl_p}fn f(input:u32)->(u32,u32){{let t=P{{{fa}:0u32,{fb}:0u32}};let s=P{{{fields},..t}};{tail}}}"
        )
    };
    let s_renamed = format!(
        "struct Q{{{fa}:u32,{fb}:u32}}fn g(input:u32)->(u32,u32){{let t=Q{{{fb}:{init_b},{fa}:{init_a}}};(t.{fa},t.{fb})}}"
    );
    let n_value = format!(
        "{decl_p}fn f(input:u32)->(u32,u32){{let s=P{{{fa}:input&{}u32,{fb}:{init_b}}};{tail}}}",
        mask + 1
    );
    let p_effect = |fields: &str| {
        format!(
            "fn eff(u:u32)->u32{{println!(\"{{u}}\");u}}{decl_p}fn f(input:u32)->(u32,u32){{let s=P{{{fields}}};{tail}}}"
        )
    };
    let observer = "macro_rules! obs{($e:expr)=>{{println!(\"{}\",$e);$e}}}";
    let p_macro = |fields: &str| {
        format!("{observer}{decl_p}fn f(input:u32)->(u32,u32){{let s=P{{{fields}}};{tail}}}")
    };
    let p_observer = |fields: &str| {
        format!(
            "#[track_caller] fn obs(u:u32)->u32{{u&{mask}u32}}{decl_p}fn f(input:u32)->(u32,u32){{let s=P{{{fields}}};{tail}}}"
        )
    };
    let n_tuple = format!(
        "struct T(u32,u32);fn f(input:u32)->(u32,u32){{let s=T({init_b},{init_a});(s.0,s.1)}}"
    );
    let p_tuple_field = |fields: &str| {
        format!(
            "struct M{{{fa}:u32,t:(u32,u32)}}fn f(input:u32)->(u32,u32){{let s=M{{{fields}}};(s.{fa},s.t.0)}}"
        )
    };
    let tuple_value = format!("({init_b},input>>{}u32)", shift + 2);
    let la = low;
    let lb = low + 1;
    let lc = low + 2;
    let arm = |alts: &str| format!("{alts}=>true,_=>false");
    let match_fn = |arms: &str| format!("fn f(input:u32)->bool{{match input{{{arms}}}}}");
    let l_renamed = format!("fn g(input:u32)->bool{{match input{{{lb}|{la}=>true,_=>false}}}}");
    let l_alias =
        format!("fn f(input:u32)->bool{{let v=input;match v{{{la}|{lb}=>true,_=>false}}}}");
    let l_alias_rev =
        format!("fn f(input:u32)->bool{{let v=input;match v{{{lb}|{la}=>true,_=>false}}}}");
    let n_guard_a = format!(
        "fn f(input:u32,flag:bool)->bool{{match input{{{la}|{lb} if flag=>true,_=>false}}}}"
    );
    let n_guard_b = format!(
        "fn f(input:u32,flag:bool)->bool{{match input{{{lb}|{la} if flag=>true,_=>false}}}}"
    );
    let n_bind = format!("fn f(input:u32)->bool{{match input{{x=>x>{la}u32,_=>false}}}}");
    let n_bind_a =
        format!("fn f(input:u32)->bool{{match input{{x@{la}|x@{lb}=>x>0u32,_=>false}}}}");
    let n_bind_b =
        format!("fn f(input:u32)->bool{{match input{{x@{lb}|x@{la}=>x>0u32,_=>false}}}}");
    let n_arm_a =
        format!("fn f(input:u32)->bool{{match input{{{la}|{lb}=>true,x=>x>{la}u32,_=>false}}}}");
    let n_arm_b =
        format!("fn f(input:u32)->bool{{match input{{x=>x>{la}u32,{la}|{lb}=>true,_=>false}}}}");
    let n_path =
        format!("const m:u32={lb};fn f(input:u32)->bool{{match input{{{la}|m=>true,_=>false}}}}");
    let n_macro_alt = format!(
        "macro_rules! lit{{()=>{{1}}}}fn f(input:u32)->bool{{match input{{lit!()|{lb}=>true,_=>false}}}}"
    );
    let n_neg = format!("fn f(input:i32)->bool{{match input{{-{la}|{lb}=>true,_=>false}}}}");
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
    let sites: Vec<DocTest> = vec![
        site(
            "s_original",
            p_body(&format!("{fa}:{init_a},{fb}:{init_b}")),
        ),
        site(
            "s_reversed",
            p_body(&format!("{fb}:{init_b},{fa}:{init_a}")),
        ),
        site("s_renamed", s_renamed),
        site("s_alias", p_alias(&format!("{fa}:{init_a},{fb}:low"))),
        site("s_alias_rev", p_alias(&format!("{fb}:low,{fa}:{init_a}"))),
        site(
            "s_helper",
            p_helper(&format!("{fa}:h(input),{fb}:{init_b}")),
        ),
        site(
            "s_helper_rev",
            p_helper(&format!("{fb}:{init_b},{fa}:h(input)")),
        ),
        site("s_base", p_base(&format!("{fa}:{init_a},{fb}:{init_b}"))),
        site(
            "s_base_rev",
            p_base(&format!("{fb}:{init_b},{fa}:{init_a}")),
        ),
        site("n_value", n_value),
        site("n_label", p_body(&format!("{fb}:{init_a},{fa}:{init_b}"))),
        site(
            "n_effect",
            p_effect(&format!("{fa}:eff(input),{fb}:{init_b}")),
        ),
        site(
            "n_effect_rev",
            p_effect(&format!("{fb}:{init_b},{fa}:eff(input)")),
        ),
        site(
            "n_macro",
            p_macro(&format!("{fa}:obs!(input),{fb}:{init_b}")),
        ),
        site(
            "n_macro_rev",
            p_macro(&format!("{fb}:{init_b},{fa}:obs!(input)")),
        ),
        site(
            "n_observer",
            p_observer(&format!("{fa}:obs(input),{fb}:{init_b}")),
        ),
        site(
            "n_observer_rev",
            p_observer(&format!("{fb}:{init_b},{fa}:obs(input)")),
        ),
        site("n_tuple", n_tuple),
        site(
            "n_tuple_field",
            p_tuple_field(&format!("{fa}:{init_a},t:{tuple_value}")),
        ),
        site(
            "n_tuple_field_rev",
            p_tuple_field(&format!("t:{tuple_value},{fa}:{init_a}")),
        ),
        site("l_original", match_fn(&arm(&format!("{la}|{lb}")))),
        site("l_reversed", match_fn(&arm(&format!("{lb}|{la}")))),
        site("l_renamed", l_renamed),
        site("l_alias", l_alias),
        site("l_alias_rev", l_alias_rev),
        site("l_dup_a", match_fn(&arm(&format!("{la}|{la}|{lb}")))),
        site("l_dup_b", match_fn(&arm(&format!("{lb}|{la}|{la}")))),
        site("l_suf_a", match_fn(&arm(&format!("{la}u32|{lb}u32")))),
        site("l_suf_b", match_fn(&arm(&format!("{lb}u32|{la}u32")))),
        site("n_litval", match_fn(&arm(&format!("{lc}|{lb}")))),
        site("n_guard_a", n_guard_a),
        site("n_guard_b", n_guard_b),
        site("n_bind", n_bind),
        site("n_bind_a", n_bind_a),
        site("n_bind_b", n_bind_b),
        site("n_arm_a", n_arm_a),
        site("n_arm_b", n_arm_b),
        site("n_path", n_path),
        site("n_neg", n_neg),
        site("n_suf_mix", match_fn(&arm(&format!("{la}u32|{lb}")))),
        site("n_macro_alt", n_macro_alt),
    ];
    let report = echoes::group(&sites, 2, 0);
    let mut groups: Vec<Vec<&str>> = report
        .groups
        .iter()
        .map(|group| {
            let mut members: Vec<_> = group.sites.iter().map(|site| site.item.as_str()).collect();
            members.sort_unstable();
            members
        })
        .collect();
    groups.sort_unstable();
    let mut expected: Vec<Vec<&str>> = vec![
        vec!["c::s_original", "c::s_renamed", "c::s_reversed"],
        vec!["c::s_alias", "c::s_alias_rev"],
        vec!["c::s_base", "c::s_base_rev"],
        vec!["c::s_helper", "c::s_helper_rev"],
        vec!["c::l_original", "c::l_renamed", "c::l_reversed"],
        vec!["c::l_alias", "c::l_alias_rev"],
        vec!["c::l_dup_a", "c::l_dup_b"],
        vec!["c::l_suf_a", "c::l_suf_b"],
    ];
    expected.sort_unstable();
    assert_eq!(groups, expected);
}

/// Closed item graphs and compatible inherent partitions preserve ordered observation edges.
pub fn check_item_schedules(data: &[u8]) {
    let byte = |index| u32::from(data.get(index).copied().unwrap_or(0));
    let (n1, n2) = if byte(0) & 1 == 0 {
        ("first", "second")
    } else {
        ("r#type", "r#loop")
    };
    let (m1, m2) = if byte(1) & 1 == 0 {
        ("one", "two")
    } else {
        ("r#type", "r#loop")
    };
    let v1 = 1 + byte(2) % 255;
    let v2 = 2 + byte(3) % 255;
    let v1_plus = v1 + 1;
    let print_pair = |x: &str, y: &str| format!("println!(\"{{:?}}\",({x},{y}));");
    let p_ab = print_pair("a", "b");
    let p_ba = print_pair("b", "a");
    let a_main = format!("fn main(){{let a={n1}();let b={n2}();{p_ab}}}");
    let a_main_scheduled = format!("fn main(){{let b={n2}();let a={n1}();{p_ab}}}");
    let a_main_port = format!("fn main(){{let b={n2}();let a={n1}();{p_ba}}}");
    let a_one = format!("fn {n1}()->u32{{{v1}u32}}");
    let a_two = format!("fn {n2}()->u32{{{v2}u32}}");
    let a_one_alpha = format!("fn alpha()->u32{{{v1}u32}}");
    let a_two_beta = format!("fn beta()->u32{{{v2}u32}}");
    let a_main_renamed = format!("fn main(){{let a=alpha();let b=beta();{p_ab}}}");
    let a_one_value = format!("fn {n1}()->u32{{{v1_plus}u32}}");
    let a_one_folded = format!("fn {n1}()->u32{{{v1}+0u32}}");
    let a_one_pub = format!("pub fn {n1}()->u32{{{v1}u32}}");
    let a_one_cfg = format!("#[cfg(all())] fn {n1}()->u32{{{v1}u32}}");
    let a_one_observer = format!("#[track_caller] fn {n1}()->u32{{{v1}u32}}");
    let a_one_macro = format!("fn {n1}()->u32{{lit!()}}");
    let a_macro = format!("macro_rules! lit{{()=>{{{v1}u32}}}}");
    let p_s = print_pair(&format!("S::{m1}()"), &format!("S::{m2}()"));
    let p_s_port = print_pair(&format!("S::{m2}()"), &format!("S::{m1}()"));
    let p_s_trait = format!("S::m();{p_s}");
    let p_t = print_pair(&format!("T::{m1}()"), &format!("T::{m2}()"));
    let p_o = print_pair(&format!("S::{m1}()"), &format!("T::{m2}()"));
    let p_g = print_pair(&format!("G::<u32>::{m1}()"), &format!("G::<u32>::{m2}()"));
    let mem1 = format!("fn {m1}()->u32{{{v1}u32}}");
    let mem2 = format!("fn {m2}()->u32{{{v2}u32}}");
    let mem1_value = format!("fn {m1}()->u32{{{v1_plus}u32}}");
    let mem1_folded = format!("fn {m1}()->u32{{{v1}+0u32}}");
    let mem1_cfg = format!("#[cfg(all())] fn {m1}()->u32{{{v1}u32}}");
    let mem1_observer = format!("#[track_caller] fn {m1}()->u32{{{v1}u32}}");
    let mem1_other = format!("fn {m1}()->u32{{{v2}u32}}");
    let extra = "fn extra()->u32{3}";
    let i1 = format!("impl S{{{mem1}}}");
    let i2 = format!("impl S{{{mem2}}}");
    let i_both = format!("impl S{{{mem1}{mem2}}}");
    let i_two_one = format!("impl S{{{mem2}{mem1}}}");
    let i1_folded = format!("impl S{{{mem1_folded}}}");
    let i1_value = format!("impl S{{{mem1_value}}}");
    let i2_extra = format!("impl S{{{mem2}{extra}}}");
    let i1_cfg = format!("impl S{{{mem1_cfg}}}");
    let i_both_cfg = format!("impl S{{{mem1_cfg}{mem2}}}");
    let i1_observer = format!("impl S{{{mem1_observer}}}");
    let i_both_observer = format!("impl S{{{mem1_observer}{mem2}}}");
    let i1_dup = format!("impl S{{{mem1_other}}}");
    let i_both_dup = format!("impl S{{{mem1}{mem1_other}}}");
    let i1_t = format!("impl T{{{mem1}}}");
    let i2_t = format!("impl T{{{mem2}}}");
    let i1_g = format!("impl<T> G<T>{{{mem1}}}");
    let i2_g = format!("impl G<u32>{{{mem2}}}");
    let p_main = format!("fn main(){{{p_s}}}");
    let p_main_port = format!("fn main(){{{p_s_port}}}");
    let p_main_trait = format!("fn main(){{{p_s_trait}}}");
    let p_main_renamed = format!("fn main(){{{p_t}}}");
    let p_main_owner = format!("fn main(){{{p_o}}}");
    let p_main_generic = format!("fn main(){{{p_g}}}");
    let trait_m = "trait M{fn m();}";
    let impl_m = "impl M for S{fn m(){}}";
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
    let sites: Vec<DocTest> = vec![
        site("a_original", format!("{a_one}{a_two}{a_main}")),
        site("a_order", format!("{a_two}{a_one}{a_main}")),
        site(
            "a_renamed",
            format!("{a_one_alpha}{a_two_beta}{a_main_renamed}"),
        ),
        site("a_scheduled", format!("{a_one}{a_two}{a_main_scheduled}")),
        site("a_composed", format!("{a_two}{a_one_folded}{a_main}")),
        site("an_value", format!("{a_one_value}{a_two}{a_main}")),
        site("an_port", format!("{a_one}{a_two}{a_main_port}")),
        site(
            "an_macro_a",
            format!("{a_macro}{a_one_macro}{a_two}{a_main}"),
        ),
        site(
            "an_macro_b",
            format!("{a_one_macro}{a_two}{a_macro}{a_main}"),
        ),
        site("an_pub_a", format!("{a_one_pub}{a_two}{a_main}")),
        site("an_pub_b", format!("{a_two}{a_one_pub}{a_main}")),
        site("an_cfg_a", format!("{a_one_cfg}{a_two}{a_main}")),
        site("an_cfg_b", format!("{a_two}{a_one_cfg}{a_main}")),
        site("an_observer_a", format!("{a_one_observer}{a_two}{a_main}")),
        site("an_observer_b", format!("{a_two}{a_one_observer}{a_main}")),
        site("i_original", format!("struct S;{i1}{i2}{p_main}")),
        site("i_combined", format!("struct S;{i_both}{p_main}")),
        site("i_block_order", format!("struct S;{i2}{i1}{p_main}")),
        site("i_member_order", format!("struct S;{i_two_one}{p_main}")),
        site(
            "i_renamed",
            format!("struct T;{i1_t}{i2_t}{p_main_renamed}"),
        ),
        site("i_composed", format!("struct S;{i2}{i1_folded}{p_main}")),
        site("pn_value", format!("struct S;{i1_value}{i2}{p_main}")),
        site("pn_port", format!("struct S;{i1}{i2}{p_main_port}")),
        site("pn_member", format!("struct S;{i1}{i2_extra}{p_main}")),
        site(
            "pn_owner",
            format!("struct S;struct T;{i1}{i2_t}{p_main_owner}"),
        ),
        site(
            "pn_generic",
            format!("struct G<T>(T);{i1_g}{i2_g}{p_main_generic}"),
        ),
        site(
            "pn_trait_a",
            format!("{trait_m}struct S;{impl_m}{i1}{i2}{p_main_trait}"),
        ),
        site(
            "pn_trait_b",
            format!("{trait_m}struct S;{i1}{i2}{impl_m}{p_main_trait}"),
        ),
        site("pn_cfg_a", format!("struct S;{i1_cfg}{i2}{p_main}")),
        site("pn_cfg_b", format!("struct S;{i_both_cfg}{p_main}")),
        site(
            "pn_observer_a",
            format!("struct S;{i1_observer}{i2}{p_main}"),
        ),
        site(
            "pn_observer_b",
            format!("struct S;{i_both_observer}{p_main}"),
        ),
        site("pn_dup_a", format!("struct S;{i1}{i1_dup}{p_main}")),
        site("pn_dup_b", format!("struct S;{i_both_dup}{p_main}")),
    ];
    let report = echoes::group(&sites, 2, 0);
    let mut groups: Vec<Vec<&str>> = report
        .groups
        .iter()
        .map(|group| {
            let mut members: Vec<_> = group.sites.iter().map(|site| site.item.as_str()).collect();
            members.sort_unstable();
            members
        })
        .collect();
    groups.sort_unstable();
    let a_group: Vec<&str> = if v1 == v2 {
        vec![
            "c::a_composed",
            "c::a_order",
            "c::a_original",
            "c::a_renamed",
            "c::a_scheduled",
            "c::an_port",
        ]
    } else {
        vec![
            "c::a_composed",
            "c::a_order",
            "c::a_original",
            "c::a_renamed",
            "c::a_scheduled",
        ]
    };
    let mut expected: Vec<Vec<&str>> = vec![
        a_group,
        vec![
            "c::i_block_order",
            "c::i_combined",
            "c::i_composed",
            "c::i_member_order",
            "c::i_original",
            "c::i_renamed",
        ],
    ];
    expected.sort_unstable();
    assert_eq!(groups, expected);
    // Coincident helper bodies are isomorphic, so the A13 slot orientation absorbs the swapped port.
    let an_port_folded = groups
        .iter()
        .any(|group| group.contains(&"c::an_port") && group.contains(&"c::a_original"));
    assert_eq!(an_port_folded, v1 == v2, "{groups:?}");
    let failing_site = |item: &str, code: String| DocTest {
        file: format!("src/{item}.rs"),
        line: 1,
        end: None,
        item: format!("c::{item}"),
        info: vec!["compile_fail".to_string()],
        code,
        allow: false,
        self_type: None,
        public: false,
    };
    let failing = echoes::group(
        &[
            failing_site("f_a_original", format!("{a_one}{a_two}{a_main}")),
            failing_site("f_a_order", format!("{a_two}{a_one}{a_main}")),
            failing_site("f_i_original", format!("struct S;{i1}{i2}{p_main}")),
            failing_site("f_i_combined", format!("struct S;{i_both}{p_main}")),
        ],
        2,
        0,
    );
    assert!(failing.groups.is_empty(), "{failing:?}");
}
