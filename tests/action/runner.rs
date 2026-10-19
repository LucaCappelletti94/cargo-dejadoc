//! A fake runner that executes the real `run` steps of `action.yml`.
//!
//! The fixture `cargo` and `gh` binaries on the `PATH` log every call, the
//! recorded `uses` steps are available for declarative checks, and each step
//! gets fresh `GITHUB_OUTPUT`, `GITHUB_ENV`, `GITHUB_PATH` and
//! `GITHUB_STEP_SUMMARY` files.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::process::Command;

use serde_json::{Value, json};

use super::expr;

const FAKE_BIN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/action/fake/bin");
const STANDARD_PATH: &[&str] = &[
    "/usr/local/sbin",
    "/usr/local/bin",
    "/usr/sbin",
    "/usr/bin",
    "/sbin",
    "/bin",
];

/// One `uses` step the runner recorded.
pub struct Use {
    pub name: String,
    pub uses: String,
    pub with: BTreeMap<String, String>,
}

/// One fake runner for one scenario.
pub struct Runner {
    steps_yaml: Vec<Value>,
    inputs: BTreeMap<String, String>,
    github: Value,
    scenario_env: BTreeMap<String, String>,
    workspace: String,
    runtmp: String,
    run_temp: String,
    fake_log: String,
    job_env: BTreeMap<String, String>,
    step_outputs: BTreeMap<String, BTreeMap<String, String>>,
    path_dirs: Vec<String>,
    steps: Vec<(String, &'static str)>,
    uses: Vec<Use>,
    summaries: Vec<String>,
    stdouts: Vec<String>,
    failed_at: Option<String>,
    exit_code: Option<i32>,
    step_seq: u32,
}

impl Runner {
    /// A runner over the parsed `action.yml`, inputs filled with the declared defaults.
    pub fn new(
        action: &Value,
        inputs: BTreeMap<String, String>,
        github: Value,
        scenario_env: BTreeMap<String, String>,
        workspace: String,
        runtmp: String,
    ) -> Self {
        let action_inputs = action.get("inputs").cloned().unwrap_or(Value::Null);
        let mut inputs = inputs;
        for key in inputs.keys() {
            assert!(
                action_inputs.get(key).is_some(),
                "action.yml has no input named {key:?}"
            );
        }
        if let Some(specs) = action_inputs.as_object() {
            for (key, spec) in specs {
                inputs.entry(key.clone()).or_insert_with(|| {
                    spec.get("default")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string()
                });
            }
        }
        let steps_yaml = action
            .get("runs")
            .and_then(Value::as_object)
            .and_then(|runs| runs.get("steps"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let run_temp = format!("{runtmp}/runner");
        let fake_log = format!("{runtmp}/fake");
        fs::create_dir_all(&run_temp).expect("make the runner temp dir");
        fs::create_dir_all(&fake_log).expect("make the fake log dir");
        Self {
            steps_yaml,
            inputs,
            github,
            scenario_env,
            workspace,
            runtmp,
            run_temp,
            fake_log,
            job_env: BTreeMap::new(),
            step_outputs: BTreeMap::new(),
            path_dirs: Vec::new(),
            steps: Vec::new(),
            uses: Vec::new(),
            summaries: Vec::new(),
            stdouts: Vec::new(),
            failed_at: None,
            exit_code: None,
            step_seq: 0,
        }
    }

    /// The step summaries joined in step order.
    pub fn summary(&self) -> String {
        self.summaries.join("\n")
    }

    pub fn uses(&self) -> &[Use] {
        &self.uses
    }

    pub fn job_env(&self) -> &BTreeMap<String, String> {
        &self.job_env
    }

    pub fn step_outputs(&self) -> &BTreeMap<String, BTreeMap<String, String>> {
        &self.step_outputs
    }

    pub fn run_temp(&self) -> &str {
        &self.run_temp
    }

    /// The posted review status file, empty when no status was written.
    pub fn status(&self) -> String {
        read(&format!("{}/dejadoc-post-status", self.run_temp))
            .trim()
            .to_string()
    }

    /// The rendered review body, empty when not rendered.
    pub fn body(&self) -> String {
        read(&format!("{}/dejadoc-review.md", self.run_temp))
    }

    pub fn gh_calls(&self) -> Vec<Value> {
        jsonl(&format!("{}/gh.jsonl", self.fake_log))
    }

    pub fn cargo_calls(&self) -> Vec<Value> {
        jsonl(&format!("{}/cargo.jsonl", self.fake_log))
    }

    /// The bodies a post referenced as `body=<file>`, resolved from the fake log dir.
    pub fn body_files(&self) -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        for call in self.gh_calls() {
            let Some(file) = call
                .get("params")
                .and_then(Value::as_object)
                .and_then(|params| params.get("body"))
                .and_then(Value::as_str)
                .and_then(|ref_| ref_.strip_prefix("body="))
            else {
                continue;
            };
            out.insert(
                file.to_string(),
                read(&format!("{}/{}", self.fake_log, file)),
            );
        }
        out
    }

    pub fn comment_posts(&self) -> Vec<Value> {
        self.gh_calls()
            .into_iter()
            .filter(|c| {
                c.get("m").and_then(Value::as_str) == Some("POST")
                    && c.get("p")
                        .and_then(Value::as_str)
                        .is_some_and(|p| p.ends_with("/comments"))
            })
            .collect()
    }

    pub fn review_posts(&self) -> Vec<Value> {
        self.gh_calls()
            .into_iter()
            .filter(|c| {
                c.get("m").and_then(Value::as_str) == Some("POST")
                    && c.get("p")
                        .and_then(Value::as_str)
                        .is_some_and(|p| p.ends_with("/reviews"))
            })
            .collect()
    }

    pub fn scan_calls(&self) -> Vec<Value> {
        self.cargo_calls()
            .into_iter()
            .filter(|c| c.get("cmd").and_then(Value::as_str) == Some("dejadoc"))
            .collect()
    }

    /// The arguments of the `which` scan call, empty when absent.
    pub fn scan_args(&self, which: usize) -> Vec<String> {
        let calls = self.scan_calls();
        let Some(call) = calls.get(which) else {
            return Vec::new();
        };
        call.get("args")
            .and_then(Value::as_array)
            .map(|args| {
                args.iter()
                    .filter_map(Value::as_str)
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The outcome of a named step, `absent` when the step does not exist.
    pub fn step_outcome(&self, name: &str) -> &str {
        self.steps
            .iter()
            .find(|(n, _)| n == name)
            .map_or("absent", |(_, outcome)| *outcome)
    }

    /// The standard output of an executed named step.
    pub fn step_stdout(&self, name: &str) -> Option<String> {
        let mut i = 0;
        for (n, outcome) in &self.steps {
            if !matches!(*outcome, "ok" | "failed") {
                continue;
            }
            if n == name {
                return self.stdouts.get(i).cloned();
            }
            i += 1;
        }
        None
    }

    /// Run every step and return the exit code of the run.
    pub fn run(&mut self) -> i32 {
        let steps: Vec<Value> = self.steps_yaml.clone();
        for step in &steps {
            let name = step
                .get("name")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .or_else(|| step.get("id").and_then(Value::as_str))
                .unwrap_or("step")
                .to_string();
            let if_text = step.get("if").and_then(Value::as_str);
            if !expr::if_true(if_text, &self.ctx()) {
                self.steps.push((name, "skipped"));
                continue;
            }
            if self.failed_at.is_some() && !expr::has_always(if_text.unwrap_or("")) {
                self.steps.push((name, "skipped-after-failure"));
                continue;
            }
            if let Some(uses) = step.get("uses").and_then(Value::as_str) {
                let mut with = BTreeMap::new();
                if let Some(withs) = step.get("with").and_then(Value::as_object) {
                    for (k, v) in withs {
                        with.insert(
                            k.clone(),
                            expr::interp(v.as_str().unwrap_or(""), &self.ctx()),
                        );
                    }
                }
                self.uses.push(Use {
                    name: name.clone(),
                    uses: uses.to_string(),
                    with,
                });
                self.steps.push((name, "used"));
                continue;
            }
            let code = self.run_step(step, &name);
            self.steps
                .push((name.clone(), if code == 0 { "ok" } else { "failed" }));
            if code != 0 {
                self.failed_at = Some(name);
                self.exit_code = Some(code);
            }
        }
        self.exit_code.unwrap_or(0)
    }

    /// The runner context an expression is evaluated against.
    fn ctx(&self) -> Value {
        let mut inputs = serde_json::Map::new();
        for (k, v) in &self.inputs {
            inputs.insert(k.clone(), Value::String(v.clone()));
        }
        let mut env = serde_json::Map::new();
        for (k, v) in &self.job_env {
            env.insert(k.clone(), Value::String(v.clone()));
        }
        let mut steps = serde_json::Map::new();
        for (k, v) in &self.step_outputs {
            let mut outputs = serde_json::Map::new();
            for (ok, ov) in v {
                outputs.insert(ok.clone(), Value::String(ov.clone()));
            }
            let mut entry = serde_json::Map::new();
            entry.insert("outputs".to_string(), Value::Object(outputs));
            steps.insert(k.clone(), Value::Object(entry));
        }
        json!({
            "inputs": Value::Object(inputs),
            "github": self.github.clone(),
            "steps": Value::Object(steps),
            "env": Value::Object(env),
            "runner": { "os": "Linux", "arch": "X64" },
            "needs": Value::Object(serde_json::Map::new()),
            "secrets": Value::Object(serde_json::Map::new()),
        })
    }

    fn run_step(&mut self, step: &Value, name: &str) -> i32 {
        let tmp = format!("{}/steps/{}", self.runtmp, self.step_seq);
        self.step_seq += 1;
        fs::create_dir_all(&tmp).expect("make the step dir");
        let mut env = self.base_env(&tmp);
        for (k, v) in &self.scenario_env {
            env.insert(k.clone(), v.clone());
        }
        for (k, v) in &self.job_env {
            env.insert(k.clone(), v.clone());
        }
        if let Some(step_env) = step.get("env").and_then(Value::as_object) {
            for (k, v) in step_env {
                env.insert(
                    k.clone(),
                    expr::interp(v.as_str().unwrap_or(""), &self.ctx()),
                );
            }
        }
        let mut dirs = vec![FAKE_BIN.to_string()];
        dirs.extend(self.path_dirs.iter().rev().cloned());
        dirs.extend(STANDARD_PATH.iter().copied().map(String::from));
        env.insert("PATH".into(), dirs.join(":"));
        fs::create_dir_all(env["HOME"].as_str()).expect("make the HOME dir");
        let mut cwd = self.workspace.clone();
        if let Some(wd) = step.get("working-directory").and_then(Value::as_str) {
            let wd = expr::interp(wd, &self.ctx());
            if !wd.is_empty() {
                let p = if Path::new(&wd).is_absolute() {
                    wd.clone()
                } else {
                    format!("{}/{}", self.workspace, wd)
                };
                fs::create_dir_all(&p).expect("make the working directory");
                cwd = p;
            }
        }
        let out = Command::new("bash")
            .arg("-c")
            .arg(step.get("run").and_then(Value::as_str).unwrap_or(""))
            .env_clear()
            .envs(&env)
            .current_dir(&cwd)
            .output()
            .expect("run the step");
        if let Some(id) = step.get("id").and_then(Value::as_str) {
            self.step_outputs
                .insert(id.to_string(), parse_kv(&read(&format!("{tmp}/output"))));
        }
        for (k, v) in parse_kv(&read(&format!("{tmp}/envfile"))) {
            self.job_env.insert(k, v);
        }
        for line in read(&format!("{tmp}/pathfile")).lines() {
            let line = line.trim();
            if !line.is_empty() {
                self.path_dirs.push(line.to_string());
            }
        }
        self.summaries.push(read(&format!("{tmp}/summary")));
        self.stdouts
            .push(String::from_utf8_lossy(&out.stdout).into_owned());
        let code = out.status.code().unwrap_or(-1);
        if code != 0 {
            let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
            text.push_str(&String::from_utf8_lossy(&out.stderr));
            let chars: Vec<char> = text.chars().collect();
            let tail: String = chars[chars.len().saturating_sub(2500)..].iter().collect();
            eprintln!("    step {name:?} rc={code}");
            for line in tail.lines() {
                eprintln!("    {line}");
            }
        }
        code
    }
    /// The fixed runner environment, before any job or step override.
    fn base_env(&self, tmp: &str) -> BTreeMap<String, String> {
        let mut env: BTreeMap<String, String> = BTreeMap::new();
        env.insert("HOME".into(), format!("{}/home", self.runtmp));
        env.insert("USER".into(), "runner".into());
        env.insert("SHELL".into(), "/bin/bash".into());
        env.insert("LANG".into(), "C.UTF-8".into());
        env.insert("RUNNER_OS".into(), "Linux".into());
        env.insert("RUNNER_ARCH".into(), "X64".into());
        env.insert("RUNNER_TEMP".into(), self.run_temp.clone());
        env.insert("GITHUB_WORKSPACE".into(), self.workspace.clone());
        env.insert(
            "GITHUB_REPOSITORY".into(),
            self.github
                .get("repository")
                .and_then(Value::as_str)
                .unwrap_or("owner/repo")
                .to_string(),
        );
        env.insert("GITHUB_SERVER_URL".into(), "https://github.com".into());
        env.insert("GITHUB_RUN_ID".into(), "1".into());
        env.insert(
            "GITHUB_EVENT_NAME".into(),
            self.github
                .get("event_name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        );
        env.insert("GITHUB_TOKEN".into(), "fake-token".into());
        env.insert("GITHUB_OUTPUT".into(), format!("{tmp}/output"));
        env.insert("GITHUB_ENV".into(), format!("{tmp}/envfile"));
        env.insert("GITHUB_PATH".into(), format!("{tmp}/pathfile"));
        env.insert("GITHUB_STEP_SUMMARY".into(), format!("{tmp}/summary"));
        env.insert("FAKE_LOG".into(), self.fake_log.clone());
        env
    }
}

fn read(path: &str) -> String {
    fs::read_to_string(path).unwrap_or_default()
}

fn jsonl(path: &str) -> Vec<Value> {
    read(path)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str(line).unwrap_or_else(|e| panic!("bad log line {line}: {e}"))
        })
        .collect()
}

/// The GitHub step file format, `name=value` lines and `name<<delim` heredocs.
fn parse_kv(text: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if line.contains("<<") && !line.contains('=') {
            let (name, delim) = line
                .split_once("<<")
                .expect("a heredoc line names a delimiter");
            i += 1;
            let mut buf = Vec::new();
            while i < lines.len() && lines[i] != delim {
                buf.push(lines[i]);
                i += 1;
            }
            out.insert(name.to_string(), buf.join("\n"));
        } else if !line.is_empty()
            && let Some((name, val)) = line.split_once('=')
        {
            out.insert(name.to_string(), val.to_string());
        }
        i += 1;
    }
    out
}
