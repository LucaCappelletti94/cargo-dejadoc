//! Bounded read-only HTTP with correlated deadlines and credentials restricted to `api.github.com`.

use alloc::format;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;
use core::fmt::Display;
use core::sync::atomic::{AtomicU64, Ordering};
use core::time::Duration;
use std::env;
use std::fs;
use std::io::{Read, Write};
use std::path::Path;
use std::process;
use std::sync::LazyLock;
use ureq::config::{Config, RedirectAuthHeaders};
use ureq::http::Response;
use ureq::{Agent, Body, Error};

/// Deadline for a bounded JSON or raw-body read.
pub(crate) const REQUEST_DEADLINE: Duration = Duration::from_secs(30);

/// Deadline for streaming a source archive to disk.
const DOWNLOAD_DEADLINE: Duration = Duration::from_secs(300);

/// Largest JSON API body a read accepts.
pub(crate) const READ_LIMIT: u64 = 16 * 1024 * 1024;

/// Largest source file a provenance read accepts.
pub(crate) const SOURCE_LIMIT: u64 = 16 * 1024 * 1024;

/// Largest archive a download accepts.
const ARCHIVE_LIMIT: u64 = 256 * 1024 * 1024;

static SEQ: AtomicU64 = AtomicU64::new(0);

/// The next process-unique correlated request identifier.
pub(crate) fn next_id() -> String {
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    format!("djd.net.{}-{seq:04}", process::id())
}

/// One line of a correlated diagnostic.
fn fail(id: &str, url: &str, cause: impl Display) -> String {
    format!("{id} GET {url} {cause}")
}

/// Whether `err` is the bounded read's refusal of an oversized body.
#[must_use]
pub(crate) fn is_oversize(err: &str) -> bool {
    err.contains("response exceeds ")
}

/// The lowercased host of `url` without a port, for `scheme://authority` URLs.
#[must_use]
pub(crate) fn host_of(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    let authority = rest.split(['/', '?', '#']).next()?.rsplit('@').next()?;
    let host = if let Some(host) = authority.strip_prefix('[') {
        host.split(']').next()?
    } else {
        authority
            .split_once(':')
            .filter(|(_, port)| port.parse::<u16>().is_ok())
            .map_or(authority, |(host, _)| host)
    };
    Some(host.to_ascii_lowercase())
}

/// Whether `url` is an https request to the `api.github.com` host.
#[must_use]
pub(crate) fn is_github_api_https(url: &str) -> bool {
    let Some(scheme) = url.split_once("://").map(|(scheme, _)| scheme) else {
        return false;
    };
    scheme.eq_ignore_ascii_case("https") && host_of(url).as_deref() == Some("api.github.com")
}

/// A `Bearer` credential from `gh_token`, falling back to `github_token`,
/// with empty values ignored.
#[must_use]
pub(crate) fn github_authorization(
    gh_token: Option<&str>,
    github_token: Option<&str>,
) -> Option<String> {
    let token = gh_token
        .filter(|token| !token.is_empty())
        .or_else(|| github_token.filter(|token| !token.is_empty()));
    token.map(|token| format!("Bearer {token}"))
}

/// The request credential for `url` read through `var`, present only for https `api.github.com`.
fn auth_for(url: &str, var: impl Fn(&str) -> Option<String>) -> Option<String> {
    if is_github_api_https(url) {
        github_authorization(var("GH_TOKEN").as_deref(), var("GITHUB_TOKEN").as_deref())
    } else {
        None
    }
}

/// Whether `status` carries a readable response body.
pub(crate) fn is_success(status: u16) -> bool {
    (200..300).contains(&status)
}

static AGENT: LazyLock<Agent> = LazyLock::new(|| {
    let config = Config::builder()
        .http_status_as_error(false)
        .redirect_auth_headers(RedirectAuthHeaders::Never)
        .build();
    Agent::new_with_config(config)
});

/// One GET with a finite deadline and the credential gate.
fn send(url: &str, deadline: Duration, id: &str) -> Result<Response<Body>, String> {
    let mut request = AGENT
        .get(url)
        .config()
        .timeout_global(Some(deadline))
        .build();
    if let Some(auth) = auth_for(url, |key| env::var(key).ok()) {
        request = request.header("Authorization", auth);
    }
    request.call().map_err(|err| {
        let cause = match &err {
            Error::Timeout(_) => "deadline exceeded".to_string(),
            _ => format!("{err}"),
        };
        fail(id, url, &cause)
    })
}

/// A bounded `GET` returning status, body, request identifier and rate-limit header.
#[must_use = "Use the response as the read output"]
pub(crate) fn get_raw(
    url: &str,
    limit: u64,
    deadline: Duration,
) -> Result<(u16, Vec<u8>, String, Option<u64>), String> {
    let id = next_id();
    let response = send(url, deadline, &id)?;
    let status = response.status().as_u16();
    let rate = response
        .headers()
        .get("x-ratelimit-remaining")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok());
    let mut body = Vec::new();
    response
        .into_body()
        .into_reader()
        .take(limit.saturating_add(1))
        .read_to_end(&mut body)
        .map_err(|err| fail(&id, url, format!("body read {err}")))?;
    let body_len = u64::try_from(body.len()).unwrap_or(u64::MAX);
    if body_len > limit {
        return Err(fail(&id, url, format!("response exceeds {limit} bytes")));
    }
    Ok((status, body, id, rate))
}

/// Fetch a bounded `JSON` document within the request deadline.
///
/// # Errors
///
/// Returns a correlated error for transport, status, size or `JSON` failures.
#[must_use = "Use the document as the read output"]
pub fn json(url: &str) -> Result<serde_json::Value, String> {
    let (status, body, id, _rate) = get_raw(url, READ_LIMIT, REQUEST_DEADLINE)?;
    if !is_success(status) {
        return Err(fail(&id, url, format!("HTTP {status}")));
    }
    serde_json::from_slice(&body).map_err(|err| fail(&id, url, format!("invalid JSON {err}")))
}

/// Download within `ARCHIVE_LIMIT` and atomically publish the complete file.
///
/// # Errors
///
/// Returns a correlated error for transport, status, size or filesystem failures.
#[must_use = "Use the result as the download outcome"]
pub fn download(url: &str, destination: &Path) -> Result<(), String> {
    let id = next_id();
    let response = send(url, DOWNLOAD_DEADLINE, &id)?;
    let status = response.status().as_u16();
    if !is_success(status) {
        return Err(fail(&id, url, format!("HTTP {status}")));
    }
    let name = destination
        .file_name()
        .ok_or_else(|| fail(&id, url, "destination has no file name"))?
        .to_string_lossy();
    let prepared = destination.parent().map_or(Ok(()), fs::create_dir_all);
    prepared.map_err(|err| fail(&id, url, format!("prepare {err}")))?;
    let part = destination.with_file_name(format!("{name}.dejadoc-part-{id}"));
    let written = match stream_to_file(response.into_body().into_reader(), &part, &id, url) {
        Ok(written) => written,
        Err(err) => {
            fs::remove_file(&part).ok();
            return Err(err);
        }
    };
    if written > ARCHIVE_LIMIT {
        fs::remove_file(&part).ok();
        return Err(fail(
            &id,
            url,
            format!("response exceeds {ARCHIVE_LIMIT} bytes"),
        ));
    }
    let renamed = fs::rename(&part, destination);
    if let Err(err) = renamed {
        fs::remove_file(&part).ok();
        return Err(fail(&id, url, format!("rename {err}")));
    }
    Ok(())
}

/// Copy `reader` to `part`, counting the bytes written.
fn stream_to_file(reader: impl Read, part: &Path, id: &str, url: &str) -> Result<u64, String> {
    let mut file = fs::File::create(part).map_err(|err| fail(id, url, format!("prepare {err}")))?;
    let mut reader = reader.take(ARCHIVE_LIMIT.saturating_add(1));
    let mut written = 0u64;
    let mut chunk = [0u8; 8 * 1024];
    loop {
        let n = reader
            .read(&mut chunk)
            .map_err(|err| fail(id, url, format!("body read {err}")))?;
        if n == 0 {
            break;
        }
        file.write_all(&chunk[..n])
            .map_err(|err| fail(id, url, format!("write {err}")))?;
        written += n as u64;
    }
    Ok(written)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use alloc::collections::BTreeMap;
    use alloc::format;
    use alloc::sync::Arc;
    use core::sync::atomic::AtomicBool;
    use std::io::{self, Write};
    use std::net::{SocketAddr, TcpListener, TcpStream};
    use std::sync::Mutex;
    use std::thread::{self, JoinHandle};
    use std::time::Instant;

    /// One fixture response body.
    #[derive(Clone)]
    enum BodySpec {
        Bytes(Vec<u8>),
        Fill(u64),
        /// Declares `n` bytes of `0xAB` and closes after sending half of them.
        Short(u64),
    }

    /// One fixture route.
    #[derive(Clone)]
    enum Route {
        Bytes(u16, BodySpec),
        Slow(u64, u16, BodySpec),
        RateLimited,
    }

    #[derive(Default)]
    struct Log {
        routes: BTreeMap<String, Route>,
        pages: BTreeMap<String, Vec<Route>>,
        auth: BTreeMap<String, String>,
        hosts: BTreeMap<String, String>,
        targets: BTreeMap<String, Vec<String>>,
        hits: BTreeMap<String, u32>,
    }

    /// A single-threaded HTTP/1.1 fixture serving real loopback sockets.
    pub(crate) struct Fixture {
        addr: SocketAddr,
        log: Arc<Mutex<Log>>,
        stop: Arc<AtomicBool>,
        thread: Mutex<Option<JoinHandle<()>>>,
    }

    impl Fixture {
        /// Start a fixture on a loopback port.
        #[must_use]
        pub(crate) fn start() -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            let log = Arc::new(Mutex::new(Log::default()));
            let stop = Arc::new(AtomicBool::new(false));
            let thread_log = Arc::clone(&log);
            let thread_stop = Arc::clone(&stop);
            let thread = thread::spawn(move || {
                let _ = listener.set_nonblocking(true);
                while !thread_stop.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            let _ = handle(stream, &thread_log);
                        }
                        Err(_) => thread::sleep(Duration::from_millis(20)),
                    }
                }
            });
            Self {
                addr,
                log,
                stop,
                thread: Mutex::new(Some(thread)),
            }
        }

        /// The API base this fixture serves under `/api`.
        #[must_use]
        pub(crate) fn api_base(&self) -> String {
            format!("http://{}/api", self.addr)
        }

        /// The raw base this fixture serves under `/raw`.
        #[must_use]
        pub(crate) fn raw_base(&self) -> String {
            format!("http://{}/raw", self.addr)
        }

        /// Serve one exact-path route.
        pub(crate) fn route(&self, path: &str, status: u16, body: &[u8]) {
            self.log.lock().unwrap().routes.insert(
                path.to_string(),
                Route::Bytes(status, BodySpec::Bytes(body.to_vec())),
            );
        }

        /// Serve one exact-path route streaming `total` bytes of `0xAB`.
        pub(crate) fn fill_route(&self, path: &str, status: u16, total: u64) {
            self.log.lock().unwrap().routes.insert(
                path.to_string(),
                Route::Bytes(status, BodySpec::Fill(total)),
            );
        }

        /// Serve one exact-path route whose body stops at half its declared `total` bytes.
        pub(crate) fn short_route(&self, path: &str, total: u64) {
            self.log
                .lock()
                .unwrap()
                .routes
                .insert(path.to_string(), Route::Bytes(200, BodySpec::Short(total)));
        }

        /// Serve one exact-path route after `sleep_ms`.
        pub(crate) fn slow_route(&self, path: &str, sleep_ms: u64, status: u16, body: &[u8]) {
            self.log.lock().unwrap().routes.insert(
                path.to_string(),
                Route::Slow(sleep_ms, status, BodySpec::Bytes(body.to_vec())),
            );
        }

        /// Serve a `403` with `x-ratelimit-remaining: 0` for one exact path.
        pub(crate) fn rate_limited_route(&self, path: &str) {
            self.log
                .lock()
                .unwrap()
                .routes
                .insert(path.to_string(), Route::RateLimited);
        }

        /// Serve queued responses in order for one exact path.
        pub(crate) fn page_route(&self, path: &str, responses: Vec<(u16, Vec<u8>)>) {
            let routes = responses
                .into_iter()
                .map(|(status, body)| Route::Bytes(status, BodySpec::Bytes(body)))
                .collect();
            self.log
                .lock()
                .unwrap()
                .pages
                .insert(path.to_string(), routes);
        }

        /// The recorded request count for one path.
        #[must_use]
        pub(crate) fn hits(&self, path: &str) -> u32 {
            *self.log.lock().unwrap().hits.get(path).unwrap_or(&0)
        }

        /// The last recorded `Authorization` value for one path.
        #[must_use]
        pub(crate) fn last_auth(&self, path: &str) -> Option<String> {
            self.log.lock().unwrap().auth.get(path).cloned()
        }

        /// The last recorded `Host` value for one path.
        #[must_use]
        pub(crate) fn last_host(&self, path: &str) -> Option<String> {
            self.log.lock().unwrap().hosts.get(path).cloned()
        }

        /// The last recorded request target, query included.
        #[must_use]
        pub(crate) fn last_target(&self, path: &str) -> Option<String> {
            self.log
                .lock()
                .unwrap()
                .targets
                .get(path)
                .and_then(|targets| targets.last().cloned())
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            let handle = self.thread.get_mut().ok().and_then(Option::take);
            let started = Instant::now();
            while handle.as_ref().is_some_and(|handle| !handle.is_finished())
                && started.elapsed() < Duration::from_secs(31)
            {
                thread::sleep(Duration::from_millis(10));
            }
            let _ = handle.filter(JoinHandle::is_finished).map(JoinHandle::join);
        }
    }

    fn handle(mut stream: TcpStream, log: &Arc<Mutex<Log>>) -> io::Result<()> {
        let deadline = Instant::now() + REQUEST_DEADLINE;
        let mut head = Vec::new();
        let mut byte = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            if head.len() > 65536 {
                return Ok(());
            }
            stream.set_read_timeout(Some(remaining(deadline)?.min(Duration::from_secs(2))))?;
            stream.read_exact(&mut byte)?;
            head.push(byte[0]);
        }
        let head = String::from_utf8_lossy(&head);
        let mut lines = head.lines();
        let request_line = lines.next().unwrap_or_default();
        let target = request_line.split_whitespace().nth(1).unwrap_or_default();
        let path = target.split('?').next().unwrap_or_default();
        let host = header_value(&head, "host").unwrap_or_default();
        let auth = header_value(&head, "authorization");
        let route = {
            let mut log = log.lock().unwrap();
            *log.hits.entry(path.to_string()).or_insert(0) += 1;
            log.hosts.insert(path.to_string(), host.to_string());
            if let Some(auth) = auth {
                log.auth.insert(path.to_string(), auth.to_string());
            }
            log.targets
                .entry(path.to_string())
                .or_default()
                .push(target.to_string());
            let static_route = log.routes.get(path).cloned();
            static_route.or_else(|| {
                let mut taken = None;
                if let Some(queue) = log.pages.get_mut(path)
                    && !queue.is_empty()
                {
                    taken = Some(queue.remove(0));
                }
                taken
            })
        };
        let Some(route) = route else {
            return write_response(
                &mut stream,
                404,
                &BodySpec::Bytes(Vec::new()),
                false,
                deadline,
            );
        };
        let (status, body, ratelimited) = match route {
            Route::Bytes(status, body) => (status, body, false),
            Route::Slow(sleep, status, body) => {
                thread::sleep(Duration::from_millis(sleep).min(remaining(deadline)?));
                (status, body, false)
            }
            Route::RateLimited => (
                403,
                BodySpec::Bytes(b"{\"message\": \"rate limit exceeded\"}".to_vec()),
                true,
            ),
        };
        write_response(&mut stream, status, &body, ratelimited, deadline)
    }

    /// The value of one request header, case-insensitive.
    fn header_value<'a>(head: &'a str, name: &str) -> Option<&'a str> {
        head.lines().find_map(|line| {
            let (key, value) = line.split_once(':')?;
            (key.trim().eq_ignore_ascii_case(name)).then(|| value.trim())
        })
    }

    fn write_response(
        stream: &mut TcpStream,
        status: u16,
        body: &BodySpec,
        ratelimited: bool,
        deadline: Instant,
    ) -> io::Result<()> {
        let (declared, total) = match body {
            BodySpec::Bytes(b) => {
                let len = u64::try_from(b.len()).unwrap_or(0);
                (len, len)
            }
            BodySpec::Fill(n) => (*n, *n),
            BodySpec::Short(n) => (*n, n / 2),
        };
        let mut head = format!(
            "HTTP/1.1 {status} {reason}\r\ncontent-type: application/octet-stream\r\ncontent-length: {declared}\r\n",
            reason = reason(status)
        );
        if ratelimited {
            head.push_str("x-ratelimit-remaining: 0\r\n");
        }
        head.push_str("connection: close\r\n\r\n");
        write_before(stream, head.as_bytes(), deadline)?;
        let fill = [0xABu8; 4096];
        let mut written = 0u64;
        let mut cursor = 0usize;
        while written < total {
            let n = usize::try_from((total - written).min(4096)).unwrap_or(0);
            let slice: &[u8] = match body {
                BodySpec::Bytes(b) => &b[cursor..cursor + n],
                BodySpec::Fill(_) | BodySpec::Short(_) => &fill[..n],
            };
            write_before(stream, slice, deadline)?;
            written += n as u64;
            cursor += n;
        }
        Ok(())
    }

    fn remaining(deadline: Instant) -> io::Result<Duration> {
        deadline
            .checked_duration_since(Instant::now())
            .filter(|duration| !duration.is_zero())
            .ok_or(io::Error::from(io::ErrorKind::TimedOut))
    }

    fn write_before(stream: &mut TcpStream, mut bytes: &[u8], deadline: Instant) -> io::Result<()> {
        while !bytes.is_empty() {
            stream.set_write_timeout(Some(remaining(deadline)?))?;
            let written = stream.write(bytes)?;
            bytes = &bytes[written..];
        }
        Ok(())
    }

    fn reason(status: u16) -> &'static str {
        match status {
            403 => "Forbidden",
            404 => "Not Found",
            429 => "Too Many Requests",
            500 => "Internal Server Error",
            _ => "OK",
        }
    }

    #[test]
    fn json_parses_bounded_response() {
        let fixture = Fixture::start();
        fixture.route("/api/ok", 200, b"{\"a\": 1}");
        let url = format!("{}/ok", fixture.api_base());
        let value = json(&url).expect("parse the fixture JSON");
        assert_eq!(value["a"], serde_json::json!(1));
        assert_eq!(
            fixture.last_host("/api/ok").as_deref(),
            Some(fixture.addr.to_string().as_str())
        );
        assert_eq!(fixture.last_auth("/api/ok"), None);
    }

    #[test]
    fn json_discloses_status_and_correlation() {
        let fixture = Fixture::start();
        fixture.route("/api/boom", 500, b"{}");
        let url = format!("{}/boom", fixture.api_base());
        let err = json(&url).unwrap_err();
        assert!(err.starts_with("djd.net."), "{err}");
        assert!(err.contains(&url), "{err}");
        assert!(err.contains("HTTP 500"), "{err}");
    }

    #[test]
    fn json_refuses_oversized_body() {
        let fixture = Fixture::start();
        fixture.fill_route("/api/big", 200, READ_LIMIT + 1);
        let err = json(&format!("{}/big", fixture.api_base())).unwrap_err();
        assert!(err.contains(&format!("exceeds {READ_LIMIT}")), "{err}");
    }

    #[test]
    fn deadline_is_finite_and_disclosed() {
        let fixture = Fixture::start();
        fixture.slow_route("/api/slow", 2_000, 200, b"{}");
        let url = format!("{}/slow", fixture.api_base());
        let err = get_raw(&url, READ_LIMIT, Duration::from_millis(300)).unwrap_err();
        assert!(err.contains("deadline exceeded"), "{err}");
    }

    #[test]
    fn download_streams_archive_atomically() {
        let fixture = Fixture::start();
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("nested");
        let destination = nested.join("dep-1.2.3.crate");
        fixture.fill_route("/raw/archive", 200, 1024 * 1024);
        download(&format!("{}/archive", fixture.raw_base()), &destination)
            .expect("stream the archive");
        let bytes = std::fs::read(&destination).expect("read the destination");
        assert_eq!(bytes.len(), 1024 * 1024);
        assert!(bytes.iter().all(|&byte| byte == 0xAB));
        assert_eq!(
            std::fs::read_dir(&nested)
                .expect("read the directory")
                .count(),
            1
        );
    }

    #[test]
    fn download_discloses_status_and_leaves_no_file() {
        let fixture = Fixture::start();
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("missing.crate");
        fixture.route("/raw/missing", 404, b"");
        let err = download(&format!("{}/missing", fixture.raw_base()), &destination).unwrap_err();
        assert!(err.contains("HTTP 404"), "{err}");
        assert!(!destination.exists());
    }

    #[test]
    fn download_refuses_oversized_archive() {
        let fixture = Fixture::start();
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("huge.crate");
        fixture.fill_route("/raw/huge", 200, ARCHIVE_LIMIT + 4096);
        let err = download(&format!("{}/huge", fixture.raw_base()), &destination).unwrap_err();
        assert!(err.contains(&format!("exceeds {ARCHIVE_LIMIT}")), "{err}");
        assert!(!destination.exists());
    }

    #[test]
    fn host_of_parses_authority() {
        assert_eq!(
            host_of("https://api.github.com:443/repo?x=1#y").as_deref(),
            Some("api.github.com")
        );
        assert_eq!(
            host_of("http://user:pass@CRATES.io/crate").as_deref(),
            Some("crates.io")
        );
        assert_eq!(host_of("http://[::1]:8080/raw").as_deref(), Some("::1"));
        assert_eq!(host_of("not-a-url"), None);
    }

    #[test]
    fn github_authorization_prefers_gh_token() {
        assert_eq!(
            github_authorization(Some("a"), Some("b")).as_deref(),
            Some("Bearer a")
        );
        assert_eq!(
            github_authorization(None, Some("b")).as_deref(),
            Some("Bearer b")
        );
        assert_eq!(
            github_authorization(Some(""), Some("b")).as_deref(),
            Some("Bearer b")
        );
        assert_eq!(github_authorization(Some(""), Some("")), None);
        assert_eq!(github_authorization(None, None), None);
    }

    #[test]
    fn auth_gate_is_https_github_api_only() {
        assert!(is_github_api_https("https://api.github.com/repos/o/r"));
        assert!(!is_github_api_https("http://api.github.com/repos/o/r"));
        assert!(!is_github_api_https("https://github.com/repos/o/r"));
        assert!(!is_github_api_https(
            "https://api.github.com.evil.com/repos/o/r"
        ));
        assert!(!is_github_api_https("not-a-url"));
    }
    #[test]
    fn send_discloses_a_transport_failure() {
        let port = {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap().port()
        };
        let url = format!("http://127.0.0.1:{port}/raw/none");
        let err = get_raw(&url, READ_LIMIT, Duration::from_secs(5)).unwrap_err();
        assert!(err.starts_with("djd.net."), "{err}");
        assert!(err.contains(&url), "{err}");
        assert!(!err.contains("deadline exceeded"), "{err}");
    }

    #[test]
    fn download_discloses_a_rename_failure() {
        let fixture = Fixture::start();
        fixture.fill_route("/raw/rename", 200, 4096);
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("dep.crate");
        std::fs::create_dir_all(&destination).unwrap();
        let err = download(&format!("{}/rename", fixture.raw_base()), &destination).unwrap_err();
        assert!(err.contains("rename"), "{err}");
        assert!(destination.is_dir());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn download_removes_the_part_on_a_prepare_failure() {
        use std::os::unix::fs::PermissionsExt;
        let fixture = Fixture::start();
        fixture.fill_route("/raw/locked", 200, 4096);
        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path().join("locked");
        std::fs::create_dir_all(&parent).unwrap();
        let original = std::fs::metadata(&parent).unwrap().permissions().mode();
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o555)).unwrap();
        let destination = parent.join("dep.crate");
        let err = download(&format!("{}/locked", fixture.raw_base()), &destination).unwrap_err();
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(original)).unwrap();
        assert!(err.contains("prepare"), "{err}");
        assert!(!destination.exists());
        assert_eq!(std::fs::read_dir(&parent).unwrap().count(), 0);
    }

    #[test]
    fn json_discloses_a_429_status() {
        let fixture = Fixture::start();
        fixture.route("/api/slow", 429, b"{}");
        let err = json(&format!("{}/slow", fixture.api_base())).unwrap_err();
        assert!(err.contains("HTTP 429"), "{err}");
    }

    #[test]
    fn fixture_drops_an_oversized_head_and_records_headers() {
        let fixture = Fixture::start();
        let mut stream = TcpStream::connect(fixture.addr).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let mut head = String::from("GET /big HTTP/1.1\r\nhost: fixture\r\nx-big: ");
        head.push_str(&"a".repeat(70_000));
        head.push_str("\r\n\r\n");
        stream.write_all(head.as_bytes()).unwrap();
        let mut got = Vec::new();
        let _ = stream.read_to_end(&mut got);
        assert!(got.is_empty(), "{got:?}");
        assert_eq!(fixture.hits("/big"), 0);
        let mut stream = TcpStream::connect(fixture.addr).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        stream
            .write_all(
                b"GET /authed HTTP/1.1\r\nhost: fixture\r\nauthorization: Bearer tok\r\n\r\n",
            )
            .unwrap();
        let mut got = Vec::new();
        stream.read_to_end(&mut got).unwrap();
        let text = String::from_utf8(got).unwrap();
        assert!(text.starts_with("HTTP/1.1 404"), "{text}");
        assert_eq!(fixture.last_auth("/authed").as_deref(), Some("Bearer tok"));
    }

    #[test]
    fn credentials_are_read_only_for_the_https_github_api() {
        let var = |key: &str| (key == "GITHUB_TOKEN").then(|| String::from("tok"));
        assert_eq!(
            auth_for("https://api.github.com/repos/o/r", var).as_deref(),
            Some("Bearer tok")
        );
        assert_eq!(auth_for("https://github.com/o/r", var), None);
        assert_eq!(auth_for("http://api.github.com/repos/o/r", var), None);
    }

    #[test]
    fn a_truncated_body_is_reported_and_leaves_no_part() {
        let fixture = Fixture::start();
        fixture.short_route("/raw/short", 64 * 1024);
        let url = format!("{}/short", fixture.raw_base());
        let err = get_raw(&url, READ_LIMIT, REQUEST_DEADLINE).unwrap_err();
        assert!(err.contains("body read"), "{err}");
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("dep.crate");
        let err = download(&url, &destination).unwrap_err();
        assert!(err.contains("body read"), "{err}");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn download_refuses_a_destination_without_a_file_name() {
        let fixture = Fixture::start();
        fixture.fill_route("/raw/any", 200, 16);
        let err = download(&format!("{}/any", fixture.raw_base()), Path::new("/")).unwrap_err();
        assert!(err.contains("destination has no file name"), "{err}");
    }
}
