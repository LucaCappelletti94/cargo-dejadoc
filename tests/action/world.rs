//! One scenario world: a workspace, fixture reports, and a runner.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};

use super::fix::report;
use super::runner::Runner;

const CARGO_TOML: &str = "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[lib]\npath = \"src/lib.rs\"\n";

pub const SRC_LIB: &str = "//!\n\n/// ```\n/// let total = value + 1u32;\n/// ```\n\npub fn parse(value: u32) -> u32 { value + 1u32 }\n";

pub const SRC_DUP: &str = "//!\n\n/// ```\n/// let total = value + 1u32;\n/// ```\n\npub fn parse(value: u32) -> u32 { value + 1u32 }\n\n// trailing\n";

pub const SRC_PARSER: &str = "//!\n\npub fn selected(value: u32) -> u32 {\n    let total = value + 1u32;\n    total * 2u32\n}\n";

/// One scenario world, set a field at a time and run it once.
pub struct World {
    tmp: tempfile::TempDir,
    workspace: PathBuf,
    base_sha: String,
    head_sha: String,
    use_git: bool,
    extra_base: Vec<(String, String)>,
    extra_head: Vec<(String, String)>,
    inputs: BTreeMap<String, String>,
    event: String,
    fork: bool,
    json: Value,
    base_json: Value,
    human: String,
    changed: Vec<String>,
    exit_code: i32,
    gh_403: bool,
    meta_root: Option<PathBuf>,
    workspace_override: Option<PathBuf>,
    runner: Option<Runner>,
}

impl World {
    /// A world with a fresh temp dir and a plain workspace.
    pub fn new(name: &str) -> Self {
        let tmp = tempfile::Builder::new()
            .prefix(&format!("echoes-scen-{name}-"))
            .tempdir_in(std::env::temp_dir())
            .expect("make the scenario dir");
        let workspace = tmp.path().join("ws");
        Self {
            tmp,
            workspace,
            base_sha: "0".repeat(40),
            head_sha: "0".repeat(40),
            use_git: false,
            extra_base: Vec::new(),
            extra_head: Vec::new(),
            inputs: BTreeMap::new(),
            event: "push".into(),
            fork: false,
            json: report(&[], &[], None, 6, 3),
            base_json: report(&[], &[], None, 6, 3),
            human: String::new(),
            changed: Vec::new(),
            exit_code: 0,
            gh_403: false,
            meta_root: None,
            workspace_override: None,
            runner: None,
        }
    }

    pub fn inputs(mut self, pairs: &[(&str, &str)]) -> Self {
        for (k, v) in pairs {
            self.inputs.insert(k.to_string(), v.to_string());
        }
        self
    }

    pub fn event(mut self, event: &str) -> Self {
        self.event = event.to_string();
        self
    }

    pub fn fork(mut self) -> Self {
        self.fork = true;
        self
    }

    pub fn git(mut self) -> Self {
        self.use_git = true;
        self
    }

    pub fn json(mut self, json: Value) -> Self {
        self.json = json;
        self
    }

    pub fn base_json(mut self, base_json: Value) -> Self {
        self.base_json = base_json;
        self
    }

    pub fn human(mut self, human: &str) -> Self {
        self.human = human.to_string();
        self
    }

    pub fn changed(mut self, files: &[&str]) -> Self {
        self.changed = files.iter().copied().map(String::from).collect();
        self
    }

    pub fn exit_code(mut self, code: i32) -> Self {
        self.exit_code = code;
        self
    }

    pub fn gh_403(mut self) -> Self {
        self.gh_403 = true;
        self
    }

    pub fn extra_base(mut self, path: &str, content: &str) -> Self {
        self.extra_base
            .push((path.to_string(), content.to_string()));
        self
    }

    pub fn workspace(mut self, workspace: PathBuf) -> Self {
        self.workspace_override = Some(workspace);
        self
    }

    pub fn base_sha(mut self, sha: &str) -> Self {
        self.base_sha = sha.to_string();
        self
    }

    pub fn head_sha(mut self, sha: &str) -> Self {
        self.head_sha = sha.to_string();
        self
    }

    /// Both commit shas of the world, for a posting job that shares it.
    pub fn shas(&self) -> (String, String) {
        (self.base_sha.clone(), self.head_sha.clone())
    }

    pub fn workspace_path(&self) -> &Path {
        &self.workspace
    }

    pub fn runner(&self) -> &Runner {
        self.runner.as_ref().expect("run the world first")
    }

    /// Write a file in the workspace at the given relative path.
    pub fn write_file(&mut self, path: &str, content: &str) {
        let dest = self.workspace.join(path);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).expect("make the file parent dir");
        }
        fs::write(&dest, content).expect("write the workspace file");
    }

    /// Root the annotation prefix at a subdirectory of the workspace.
    pub fn set_meta_root(&mut self, path: PathBuf) {
        self.meta_root = Some(path);
    }

    /// Build the fixtures, run every step, and return the exit code.
    pub fn run(&mut self) -> i32 {
        if let Some(ws) = self.workspace_override.clone() {
            self.workspace = ws;
        }
        if self.use_git {
            let (workspace, base, head) =
                git_workspace(self.tmp.path(), &self.extra_base, &self.extra_head);
            self.workspace = workspace;
            self.base_sha = base;
            self.head_sha = head;
        } else {
            fs::create_dir_all(self.workspace.join("src")).expect("make the workspace src dir");
        }
        let fixtures = self.tmp.path().join("fixtures");
        fs::create_dir_all(&fixtures).expect("make the fixtures dir");
        fs::write(
            fixtures.join("report.json"),
            serde_json::to_string(&self.json).expect("serialize the report"),
        )
        .expect("write the report fixture");
        fs::write(
            fixtures.join("base.json"),
            serde_json::to_string(&self.base_json).expect("serialize the base report"),
        )
        .expect("write the base fixture");
        fs::write(fixtures.join("human.txt"), &self.human).expect("write the human fixture");
        fs::write(fixtures.join("changed.txt"), self.changed.join("\n"))
            .expect("write the changed fixture");
        let mut scenario_env = BTreeMap::new();
        for (key, file) in [
            ("SCEN_JSON", "report.json"),
            ("SCEN_BASE_JSON", "base.json"),
            ("SCEN_HUMAN", "human.txt"),
            ("SCEN_CHANGED_FILES", "changed.txt"),
        ] {
            scenario_env.insert(
                key.to_string(),
                fixtures.join(file).to_string_lossy().into_owned(),
            );
        }
        scenario_env.insert("SCEN_HEAD_SHA".into(), self.head_sha.clone());
        scenario_env.insert("SCEN_BASE_SHA".into(), self.base_sha.clone());
        scenario_env.insert("SCEN_EXIT".into(), self.exit_code.to_string());
        scenario_env.insert(
            "SCEN_GH_403".into(),
            if self.gh_403 { "1".into() } else { "0".into() },
        );
        scenario_env.insert(
            "SCEN_META_ROOT".into(),
            self.meta_root
                .clone()
                .unwrap_or_else(|| self.workspace.clone())
                .to_string_lossy()
                .into_owned(),
        );
        let action_text =
            fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("action.yml"))
                .expect("read action.yml");
        let action: Value = serde_norway::from_str(&action_text).expect("parse action.yml");
        let runtmp = self.tmp.path().join("runtmp");
        fs::create_dir_all(&runtmp).expect("make the runtmp dir");
        let mut runner = Runner::new(
            &action,
            self.inputs.clone(),
            github_fixture(&self.event, self.fork),
            scenario_env,
            self.workspace.to_string_lossy().into_owned(),
            runtmp.to_string_lossy().into_owned(),
        );
        let code = runner.run();
        self.runner = Some(runner);
        code
    }
}

/// The fake `github` context of a scenario.
fn github_fixture(event: &str, fork: bool) -> Value {
    json!({
        "repository": "owner/repo",
        "event_name": event,
        "ref": if event == "pull_request" { "refs/pull/7/head" } else { "refs/heads/main" },
        "run_id": "12345",
        "run_attempt": "1",
        "event": {
            "pull_request": {
                "number": 7,
                "head": {
                    "repo": {
                        "full_name": if fork { "fork/repo" } else { "owner/repo" }
                    }
                }
            }
        }
    })
}

/// A cloned repository with a pushed base commit and an unpushed head commit.
fn git_workspace(
    tmp: &Path,
    extra_base: &[(String, String)],
    extra_head: &[(String, String)],
) -> (PathBuf, String, String) {
    let origin = tmp.join("origin.git");
    let ws = tmp.join("ws");
    git(&[
        "init".into(),
        "-q".into(),
        "-b".into(),
        "master".into(),
        "--bare".into(),
        origin.display().to_string(),
    ]);
    git(&[
        "clone".into(),
        "-q".into(),
        origin.display().to_string(),
        ws.display().to_string(),
    ]);
    git(&[
        "-C".into(),
        ws.display().to_string(),
        "config".into(),
        "user.email".into(),
        "t@example.com".into(),
    ]);
    git(&[
        "-C".into(),
        ws.display().to_string(),
        "config".into(),
        "user.name".into(),
        "t".into(),
    ]);
    let mut files = BTreeMap::new();
    files.insert("Cargo.toml".to_string(), CARGO_TOML.to_string());
    files.insert("src/lib.rs".to_string(), SRC_LIB.to_string());
    for (path, content) in extra_base {
        files.insert(path.clone(), content.clone());
    }
    for (path, content) in &files {
        write_file_raw(&ws, path, content);
    }
    git(&[
        "-C".into(),
        ws.display().to_string(),
        "add".into(),
        "-A".into(),
    ]);
    git(&[
        "-C".into(),
        ws.display().to_string(),
        "commit".into(),
        "-q".into(),
        "-m".into(),
        "base".into(),
    ]);
    let base = rev_parse(&ws);
    git(&[
        "-C".into(),
        ws.display().to_string(),
        "push".into(),
        "-q".into(),
        "origin".into(),
        "master".into(),
    ]);
    let mut head_files: Vec<(String, String)> =
        vec![("src/dup.rs".to_string(), SRC_DUP.to_string())];
    for (path, content) in extra_head {
        head_files.push((path.clone(), content.clone()));
    }
    for (path, content) in &head_files {
        write_file_raw(&ws, path, content);
    }
    git(&[
        "-C".into(),
        ws.display().to_string(),
        "add".into(),
        "-A".into(),
    ]);
    git(&[
        "-C".into(),
        ws.display().to_string(),
        "commit".into(),
        "-q".into(),
        "-m".into(),
        "head".into(),
    ]);
    let head = rev_parse(&ws);
    (ws, base, head)
}

fn git(args: &[String]) {
    let out = Command::new("git").args(args).output().expect("run git");
    assert!(
        out.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr)
    );
}

fn rev_parse(ws: &Path) -> String {
    let out = Command::new("git")
        .args(["-C", &ws.display().to_string(), "rev-parse", "HEAD"])
        .output()
        .expect("run git");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn write_file_raw(root: &Path, path: &str, content: &str) {
    let dest = root.join(path);
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).expect("make the file parent dir");
    }
    fs::write(&dest, content).expect("write the workspace file");
}
