use std::path::Path;
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

static REQUEST: AtomicU64 = AtomicU64::new(0);

struct Process(Child);

impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(5) {
            if !matches!(self.0.try_wait(), Ok(None)) {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
}

pub(super) fn run_cli(root: &Path, args: &[&str]) -> Output {
    let id = format!(
        "dejadoc.test.{}-{}",
        std::process::id(),
        REQUEST.fetch_add(1, Ordering::Relaxed)
    );
    let deadline = Instant::now() + Duration::from_secs(120);
    let capture = tempfile::tempdir().unwrap();
    let stdout = capture.path().join(format!("{id}.stdout"));
    let stderr = capture.path().join(format!("{id}.stderr"));
    let mut process = Process(
        Command::new(env!("CARGO_BIN_EXE_cargo-dejadoc"))
            .current_dir(root)
            .args(args)
            .env("DEJADOC_TEST_REQUEST", &id)
            .stdin(Stdio::null())
            .stdout(std::fs::File::create(&stdout).unwrap())
            .stderr(std::fs::File::create(&stderr).unwrap())
            .spawn()
            .unwrap_or_else(|error| panic!("{id} spawn failed {error}")),
    );
    loop {
        match process.0.try_wait() {
            Ok(Some(status)) => {
                return Output {
                    status,
                    stdout: std::fs::read(stdout).unwrap(),
                    stderr: std::fs::read(stderr).unwrap(),
                };
            }
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            Ok(None) => panic!("{id} CLI deadline expired"),
            Err(error) => panic!("{id} wait failed {error}"),
        }
    }
}
