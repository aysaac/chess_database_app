//! A real server on an ephemeral port of 127.0.0.1, with a temporary data
//! directory, and a blocking HTTP client for it.

#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use chess_server::AppState;
use chess_server::config::Config;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

pub const PASSWORD: &str = "a good password";

pub fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../testdata/pgn").join(name)
}

pub fn fixture_bytes(name: &str) -> Vec<u8> {
    std::fs::read(fixture(name)).expect("fixture")
}

pub struct TestServer {
    pub base: String,
    pub dir: tempfile::TempDir,
    pub state: Arc<AppState>,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl TestServer {
    pub fn start() -> TestServer {
        TestServer::with(|_| {})
    }

    pub fn with(change: impl FnOnce(&mut Config)) -> TestServer {
        TestServer::start_in(tempfile::tempdir().expect("temp dir"), change)
    }

    /// Stops the server and starts another on the same data directory.
    pub fn restart(mut self) -> TestServer {
        self.stop_now();
        let dir = std::mem::replace(&mut self.dir, tempfile::tempdir().expect("temp dir"));
        TestServer::start_in(dir, |_| {})
    }

    fn start_in(dir: tempfile::TempDir, change: impl FnOnce(&mut Config)) -> TestServer {
        let mut config = Config::new(dir.path().join("data"));
        // Tests poll fast; a test of the limits sets them.
        config.rate_user = None;
        config.rate_anon = None;
        change(&mut config);
        let state = AppState::new(config).expect("server state");
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let (addr_tx, addr_rx) = std::sync::mpsc::channel();
        let st = state.clone();
        let thread = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .expect("runtime");
            rt.block_on(async move {
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
                addr_tx.send(listener.local_addr().expect("address")).expect("send address");
                chess_server::serve(listener, st, async {
                    let _ = stopped.await;
                })
                .await
                .expect("serve");
            });
        });
        let addr = addr_rx.recv().expect("server address");
        TestServer { base: format!("http://{addr}"), dir, state, stop: Some(stop), thread: Some(thread) }
    }

    pub fn data_dir(&self) -> PathBuf {
        self.state.config.data_dir.clone()
    }

    pub fn client(&self) -> Client {
        Client::new(&self.base)
    }

    /// The first admin, made with the setup token, logged in.
    pub fn admin(&self) -> Client {
        let token = self.state.setup_token().expect("no users yet");
        let c = self.client();
        let r = c.post("/api/setup", json!({ "token": token, "username": "admin", "password": PASSWORD }));
        assert_eq!(r.status, 201, "{}", r.body);
        let mut c = self.client();
        c.login("admin", PASSWORD).ok();
        c
    }

    /// A new user (made by `admin`), logged in.
    pub fn user(&self, admin: &Client, name: &str) -> Client {
        let r = admin.post("/api/admin/users", json!({ "username": name, "password": PASSWORD }));
        assert_eq!(r.status, 201, "{}", r.body);
        let mut c = self.client();
        c.login(name, PASSWORD).ok();
        c
    }
}

impl TestServer {
    fn stop_now(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.stop_now();
    }
}

#[derive(Debug)]
pub struct Res {
    pub status: u16,
    pub body: String,
    pub headers: Vec<(String, String)>,
}

impl Res {
    pub fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or_else(|e| panic!("not JSON ({e}): {}", self.body))
    }

    pub fn parse<T: DeserializeOwned>(&self) -> T {
        serde_json::from_str(&self.body).unwrap_or_else(|e| panic!("unexpected JSON ({e}): {}", self.body))
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    pub fn ok(self) -> Res {
        assert!((200..300).contains(&self.status), "status {}: {}", self.status, self.body);
        self
    }

    pub fn code(&self) -> String {
        self.json()["error"]["code"].as_str().unwrap_or_default().to_owned()
    }
}

pub struct Client {
    base: String,
    agent: ureq::Agent,
    pub cookie: Option<String>,
    pub csrf: Option<String>,
    pub bearer: Option<String>,
    /// Sent as `Origin` on every request.
    pub origin: Option<String>,
}

impl Client {
    pub fn new(base: &str) -> Client {
        let agent = ureq::Agent::config_builder().http_status_as_error(false).build().into();
        Client { base: base.to_owned(), agent, cookie: None, csrf: None, bearer: None, origin: None }
    }

    /// The same session, as another client value.
    pub fn same(&self) -> Client {
        Client {
            base: self.base.clone(),
            agent: self.agent.clone(),
            cookie: self.cookie.clone(),
            csrf: self.csrf.clone(),
            bearer: self.bearer.clone(),
            origin: self.origin.clone(),
        }
    }

    pub fn with_bearer(&self, token: &str) -> Client {
        Client { bearer: Some(token.to_owned()), cookie: None, csrf: None, ..self.same() }
    }

    fn headers<B>(&self, mut req: ureq::RequestBuilder<B>, csrf: bool) -> ureq::RequestBuilder<B> {
        if let Some(c) = &self.cookie {
            req = req.header("Cookie", format!("chessdb_session={c}"));
        }
        if let (true, Some(t)) = (csrf, &self.csrf) {
            req = req.header("X-CSRF-Token", t);
        }
        if let Some(b) = &self.bearer {
            req = req.header("Authorization", format!("Bearer {b}"));
        }
        if let Some(o) = &self.origin {
            req = req.header("Origin", o);
        }
        req
    }

    fn finish(result: Result<ureq::http::Response<ureq::Body>, ureq::Error>) -> Res {
        let mut resp = result.expect("request");
        let headers = resp
            .headers()
            .iter()
            .map(|(k, v)| (k.as_str().to_owned(), v.to_str().unwrap_or_default().to_owned()))
            .collect();
        let status = resp.status().as_u16();
        let body = resp.body_mut().with_config().limit(64 << 20).read_to_string().unwrap_or_default();
        Res { status, body, headers }
    }

    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    pub fn get(&self, path: &str) -> Res {
        Client::finish(self.headers(self.agent.get(self.url(path)), false).call())
    }

    /// A GET that may find no server listening (None then).
    pub fn try_get(&self, path: &str) -> Option<Res> {
        let resp = self.headers(self.agent.get(self.url(path)), false).call().ok()?;
        Some(Client::finish(Ok(resp)))
    }

    pub fn delete(&self, path: &str) -> Res {
        Client::finish(self.headers(self.agent.delete(self.url(path)), true).call())
    }

    fn with_body(&self, method: &str, path: &str, body: &Value) -> Res {
        let url = self.url(path);
        let req = match method {
            "POST" => self.agent.post(url),
            "PUT" => self.agent.put(url),
            _ => self.agent.patch(url),
        };
        Client::finish(self.headers(req, true).content_type("application/json").send(body.to_string()))
    }

    pub fn post(&self, path: &str, body: Value) -> Res {
        self.with_body("POST", path, &body)
    }

    pub fn put(&self, path: &str, body: Value) -> Res {
        self.with_body("PUT", path, &body)
    }

    pub fn patch(&self, path: &str, body: Value) -> Res {
        self.with_body("PATCH", path, &body)
    }

    /// POSTs `bytes` as the body (with a Content-Length).
    pub fn upload(&self, path: &str, bytes: &[u8]) -> Res {
        let req = self.headers(self.agent.post(self.url(path)), true).content_type("application/x-chess-pgn");
        Client::finish(req.send(bytes))
    }

    /// POSTs `bytes` chunked, without a Content-Length.
    pub fn upload_chunked(&self, path: &str, bytes: &[u8]) -> Res {
        let req = self.headers(self.agent.post(self.url(path)), true).content_type("application/x-chess-pgn");
        let mut reader = bytes;
        Client::finish(req.send(ureq::SendBody::from_reader(&mut reader)))
    }

    /// Logs in; keeps the session cookie and the CSRF token.
    pub fn login(&mut self, username: &str, password: &str) -> Res {
        let r = self.post("/api/login", json!({ "username": username, "password": password }));
        if r.status == 200 {
            let cookie = r.header("set-cookie").expect("a session cookie");
            let value = cookie.split(';').next().unwrap().split_once('=').unwrap().1;
            self.cookie = Some(value.to_owned());
            self.csrf = r.json()["csrfToken"].as_str().map(str::to_owned);
        }
        r
    }

    /// Makes a database; its id.
    pub fn create_db(&self, name: &str, visibility: &str) -> String {
        let r = self.post("/api/databases", json!({ "name": name, "visibility": visibility })).ok();
        r.json()["id"].as_str().unwrap().to_owned()
    }

    /// Polls a job until it ends; its last state.
    pub fn wait_job(&self, job: u64) -> Value {
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            let r = self.get(&format!("/api/jobs/{job}")).ok();
            let v = r.json();
            if v["state"] != "running" {
                return v;
            }
            assert!(Instant::now() < deadline, "job {job} still runs");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Uploads a fixture into `db` and waits for the import; the job.
    pub fn import_fixture(&self, db: &str, name: &str) -> Value {
        let r = self.upload(&format!("/api/databases/{db}/import?filename={name}"), &fixture_bytes(name));
        assert_eq!(r.status, 202, "{}", r.body);
        let job = self.wait_job(r.json()["job"].as_u64().unwrap());
        assert_eq!(job["state"], "done", "{job}");
        job
    }
}
