//! Server settings: command-line flags, environment variables and defaults
//! (`docs/server.md` lists them).

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Default address: this machine only. The container sets `0.0.0.0:8080`.
pub const DEFAULT_BIND: &str = "127.0.0.1:8080";
/// Default largest PGN upload: 256 MiB.
pub const DEFAULT_MAX_UPLOAD: u64 = 256 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct Config {
    /// The data directory (a volume in the container): the metadata store,
    /// the databases, the import folder and uploads in progress.
    pub data_dir: PathBuf,
    pub bind: SocketAddr,
    /// Largest PGN upload, in bytes.
    pub max_upload: u64,
    /// Where users reach the server, e.g. `https://chess.example.com`. An
    /// `https` URL makes the session cookie `Secure`; the URL's origin is
    /// the one state-changing requests must come from.
    pub public_url: Option<String>,
    /// Trust `X-Forwarded-For` and `X-Forwarded-Proto` from a reverse proxy
    /// in front (Caddy in the example compose file). Off when the server is
    /// reachable directly, or anyone could pick their address.
    pub trust_proxy: bool,
    /// How long a browser session lasts.
    pub session_lifetime: Duration,
    /// Failed logins allowed per account and per client address in
    /// `login_window` before further attempts are refused until it passes.
    pub login_attempts: u32,
    pub login_window: Duration,
    /// Password hashes computed at once (each Argon2id hash takes 19 MiB).
    pub hash_concurrency: usize,
    /// Anyone may make an account without an invite (off by default).
    pub open_signup: bool,
    /// The quota of an account someone made without an invite.
    pub signup_quota: Option<u64>,
    /// How long an invite works, by default.
    pub invite_lifetime: Duration,
    /// How long a reset link works: one the admin made, one sent by e-mail.
    pub reset_lifetime: Duration,
    pub email_reset_lifetime: Duration,
    /// Where to send e-mail from ("Forgot password"), if anywhere.
    pub smtp: Option<SmtpConfig>,
}

/// How the server sends e-mail (password reset links).
#[derive(Debug, Clone)]
pub struct SmtpConfig {
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
    /// e.g. `Chess server <chess@example.com>`.
    pub from: String,
    pub tls: SmtpTls,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmtpTls {
    /// Plain connection upgraded with STARTTLS (port 587), the default.
    StartTls,
    /// TLS from the start (port 465).
    Tls,
    /// No TLS: only for a relay on the same machine or network.
    None,
}

/// Default quota of an account made without an invite: 1 GiB.
pub const DEFAULT_SIGNUP_QUOTA: u64 = 1024 * 1024 * 1024;

impl Config {
    pub fn new(data_dir: PathBuf) -> Config {
        Config {
            data_dir,
            bind: DEFAULT_BIND.parse().expect("valid default address"),
            max_upload: DEFAULT_MAX_UPLOAD,
            public_url: None,
            trust_proxy: false,
            session_lifetime: Duration::from_secs(14 * 24 * 3600),
            login_attempts: 5,
            login_window: Duration::from_secs(15 * 60),
            hash_concurrency: 4,
            open_signup: false,
            signup_quota: Some(DEFAULT_SIGNUP_QUOTA),
            invite_lifetime: Duration::from_secs(7 * 24 * 3600),
            reset_lifetime: Duration::from_secs(24 * 3600),
            email_reset_lifetime: Duration::from_secs(3600),
            smtp: None,
        }
    }

    /// Whether "Forgot password" can e-mail a link: SMTP is set up, and the
    /// public URL the link points to.
    pub fn email_resets(&self) -> bool {
        self.smtp.is_some() && self.public_url.is_some()
    }

    /// The metadata store: users, sessions, tokens, databases, shares.
    pub fn store_path(&self) -> PathBuf {
        self.data_dir.join("server.sqlite")
    }

    /// The databases, as `<id>.chessdb/`.
    pub fn dbs_dir(&self) -> PathBuf {
        self.data_dir.join("dbs")
    }

    /// Files the admin can import from (PGN, ChessBase `.cbh`).
    pub fn import_dir(&self) -> PathBuf {
        self.data_dir.join("import")
    }

    /// Uploads being received or imported; emptied at start.
    pub fn tmp_dir(&self) -> PathBuf {
        self.data_dir.join("tmp")
    }

    /// Whether the session cookie gets `Secure`: the public URL is https.
    pub fn secure_cookies(&self) -> bool {
        self.public_url.as_deref().is_some_and(|u| u.starts_with("https://"))
    }

    /// The scheme and host of [`public_url`](Self::public_url), as an
    /// `Origin` header gives them.
    pub fn public_origin(&self) -> Option<String> {
        let url = self.public_url.as_deref()?;
        let (scheme, rest) = url.split_once("://")?;
        let host = rest.split('/').next()?;
        Some(format!("{scheme}://{host}").to_ascii_lowercase())
    }

    /// Reads the settings from the environment, then `args` (flags win).
    pub fn from_env_and_args(args: &mut Vec<String>) -> Result<Config, String> {
        let env = |name: &str| std::env::var(name).ok().filter(|v| !v.trim().is_empty());
        let data = take_flag(args, "--data")?
            .or_else(|| env("CHESS_SERVER_DATA"))
            .unwrap_or_else(|| "data".to_owned());
        let mut config = Config::new(PathBuf::from(data));
        if let Some(bind) = take_flag(args, "--bind")?.or_else(|| env("CHESS_SERVER_BIND")) {
            config.bind =
                bind.parse().map_err(|_| format!("not an address: {bind} (e.g. 127.0.0.1:8080)"))?;
        }
        if let Some(mb) = env("CHESS_SERVER_MAX_UPLOAD_MB") {
            let mb: u64 =
                mb.parse().map_err(|_| format!("CHESS_SERVER_MAX_UPLOAD_MB: not a number: {mb}"))?;
            config.max_upload = mb.saturating_mul(1024 * 1024);
        }
        config.public_url = env("CHESS_SERVER_PUBLIC_URL").map(|u| u.trim_end_matches('/').to_owned());
        config.trust_proxy =
            env("CHESS_SERVER_TRUST_PROXY").is_some_and(|v| matches!(v.as_str(), "1" | "true"));
        config.open_signup =
            env("CHESS_SERVER_OPEN_SIGNUP").is_some_and(|v| matches!(v.as_str(), "1" | "true"));
        if let Some(mb) = env("CHESS_SERVER_SIGNUP_QUOTA_MB") {
            let mb: u64 =
                mb.parse().map_err(|_| format!("CHESS_SERVER_SIGNUP_QUOTA_MB: not a number: {mb}"))?;
            config.signup_quota = (mb > 0).then(|| mb.saturating_mul(1024 * 1024));
        }
        if let Some(host) = env("CHESS_SERVER_SMTP_HOST") {
            let tls = match env("CHESS_SERVER_SMTP_TLS").as_deref().unwrap_or("starttls") {
                "starttls" => SmtpTls::StartTls,
                "tls" => SmtpTls::Tls,
                "none" => SmtpTls::None,
                other => return Err(format!("CHESS_SERVER_SMTP_TLS: starttls, tls or none, not {other}")),
            };
            let default_port = match tls {
                SmtpTls::Tls => 465,
                SmtpTls::StartTls => 587,
                SmtpTls::None => 25,
            };
            let port = match env("CHESS_SERVER_SMTP_PORT") {
                Some(p) => p.parse().map_err(|_| format!("CHESS_SERVER_SMTP_PORT: not a port: {p}"))?,
                None => default_port,
            };
            let from = env("CHESS_SERVER_SMTP_FROM").ok_or(
                "CHESS_SERVER_SMTP_FROM is needed with CHESS_SERVER_SMTP_HOST (the sender's address)",
            )?;
            config.smtp = Some(SmtpConfig {
                host,
                port,
                username: env("CHESS_SERVER_SMTP_USER"),
                password: env("CHESS_SERVER_SMTP_PASSWORD"),
                from,
                tls,
            });
            if config.public_url.is_none() {
                log::warn!(
                    "CHESS_SERVER_SMTP_HOST is set but not CHESS_SERVER_PUBLIC_URL: no reset links by e-mail"
                );
            }
        }
        Ok(config)
    }
}

/// Removes `--name VALUE` or `--name=VALUE` from `args`.
fn take_flag(args: &mut Vec<String>, name: &str) -> Result<Option<String>, String> {
    let prefix = format!("{name}=");
    if let Some(i) = args.iter().position(|a| a.starts_with(&prefix)) {
        let value = args.remove(i)[prefix.len()..].to_owned();
        return Ok(Some(value));
    }
    match args.iter().position(|a| a == name) {
        Some(i) if i + 1 < args.len() => {
            args.remove(i);
            Ok(Some(args.remove(i)))
        }
        Some(_) => Err(format!("{name} needs a value")),
        None => Ok(None),
    }
}

/// Creates the data directory's folders.
pub fn prepare_data_dir(config: &Config) -> std::io::Result<()> {
    for dir in [config.data_dir.clone(), config.dbs_dir(), config.import_dir(), config.tmp_dir()] {
        std::fs::create_dir_all(&dir)?;
    }
    // Uploads left by a crash or a restart are never imported.
    clear_dir(&config.tmp_dir())?;
    // A database deleted while a request still had it open (`Pool::remove`).
    for entry in std::fs::read_dir(config.dbs_dir())? {
        let path = entry?.path();
        if path.extension().is_some_and(|e| e == "deleted") {
            log::info!("removing deleted database {}", path.display());
            std::fs::remove_dir_all(&path)?;
        }
    }
    Ok(())
}

fn clear_dir(dir: &Path) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            std::fs::remove_dir_all(entry.path())?;
        } else {
            std::fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_are_taken_out() {
        let mut args: Vec<String> = ["serve", "--data", "/x", "--bind=0.0.0.0:9"].map(String::from).to_vec();
        assert_eq!(take_flag(&mut args, "--data").unwrap().as_deref(), Some("/x"));
        assert_eq!(take_flag(&mut args, "--bind").unwrap().as_deref(), Some("0.0.0.0:9"));
        assert_eq!(args, ["serve"]);
        let mut args = vec!["--data".to_owned()];
        assert!(take_flag(&mut args, "--data").is_err());
    }

    #[test]
    fn public_origin_and_secure_cookies() {
        let mut c = Config::new(PathBuf::from("d"));
        assert_eq!(c.public_origin(), None);
        assert!(!c.secure_cookies());
        c.public_url = Some("https://Chess.Example.com/sub".into());
        assert_eq!(c.public_origin().as_deref(), Some("https://chess.example.com"));
        assert!(c.secure_cookies());
    }
}
