//! `chess-server`: the self-hosted server (NET-03). See `docs/server.md`.

use std::io::{BufRead, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::process::ExitCode;
use std::time::Duration;

use chess_server::config::Config;
use chess_server::secret::{check_new_password, hash_password};
use chess_server::store::Store;
use chess_server::wire::Role;
use chess_server::{AppState, serve};

const USAGE: &str = "\
chess-server: several users' chess databases, and searches on them

Usage:
  chess-server [serve]               Run the server
  chess-server admin create NAME     Make an admin account
  chess-server admin password NAME   Set a user's password
  chess-server healthcheck           Exit 0 if the server answers (for the container)

Options (or environment variables):
  --data DIR    CHESS_SERVER_DATA       Data directory (default ./data)
  --bind ADDR   CHESS_SERVER_BIND       Address to listen on (default 127.0.0.1:8080)
                CHESS_SERVER_MAX_UPLOAD_MB  Largest PGN upload (default 256)
                CHESS_SERVER_PUBLIC_URL     e.g. https://chess.example.com (Secure cookies, Origin check)
                CHESS_SERVER_TRUST_PROXY    1 behind a reverse proxy (X-Forwarded-For/-Proto)
                RUST_LOG                    Log level (default info)

`admin` reads the password from CHESS_SERVER_PASSWORD, or else the first line of
standard input (it is shown as you type: pipe it in, or use the setup token the
server prints at its first start instead).
";

fn main() -> ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("chess-server {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    let config = match Config::from_env_and_args(&mut args) {
        Ok(c) => c,
        Err(e) => return fail(&e),
    };
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    let result = match words.as_slice() {
        [] | ["serve"] => run(config),
        ["admin", "create", name] => set_user(&config, name, true),
        ["admin", "password", name] => set_user(&config, name, false),
        ["healthcheck"] => healthcheck(config.bind),
        _ => Err(format!("unknown command: {}\n\n{USAGE}", args.join(" "))),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => fail(&e),
    }
}

fn fail(message: &str) -> ExitCode {
    eprintln!("chess-server: {message}");
    ExitCode::FAILURE
}

fn run(config: Config) -> Result<(), String> {
    let runtime =
        tokio::runtime::Builder::new_multi_thread().enable_all().build().map_err(|e| e.to_string())?;
    runtime.block_on(async move {
        let bind = config.bind;
        let state = AppState::new(config)?;
        let listener =
            tokio::net::TcpListener::bind(bind).await.map_err(|e| format!("listen on {bind}: {e}"))?;
        log::info!("listening on http://{bind} (data in {})", state.config.data_dir.display());
        serve(listener, state, shutdown_signal()).await.map_err(|e| e.to_string())?;
        log::info!("stopped");
        Ok(())
    })
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! {
        () = ctrl_c => {}
        () = term => {}
    }
    log::info!("shutting down: stopping the running jobs");
}

fn read_password() -> Result<String, String> {
    if let Ok(p) = std::env::var("CHESS_SERVER_PASSWORD") {
        return Ok(p);
    }
    eprint!("Password: ");
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line).map_err(|e| e.to_string())?;
    Ok(line.trim_end_matches(['\r', '\n']).to_owned())
}

/// Makes admin `name` (`create`), or sets the password of user `name`.
fn set_user(config: &Config, name: &str, create: bool) -> Result<(), String> {
    std::fs::create_dir_all(&config.data_dir).map_err(|e| format!("{}: {e}", config.data_dir.display()))?;
    let store = Store::open(&config.store_path()).map_err(|e| e.to_string())?;
    let existing = store.user_by_name(name).map_err(|e| e.message)?;
    if create && existing.is_some() {
        return Err(format!("a user called {name} exists already (use `admin password {name}`)"));
    }
    if !create && existing.is_none() {
        return Err(format!("no user is called {name}"));
    }
    let password = read_password()?;
    check_new_password(&password)?;
    let hash = hash_password(&password)?;
    match existing {
        None => {
            if !(1..=32).contains(&name.len())
                || !name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
            {
                return Err("a username has 1 to 32 letters, digits, '.', '_' or '-'".into());
            }
            store.create_user(name, &hash, Role::Admin).map_err(|e| e.message)?;
            eprintln!("Made admin {name}.");
        }
        Some((user, _)) => {
            store.set_password(user.id, &hash).map_err(|e| e.message)?;
            store.delete_sessions_of(user.id, None).map_err(|e| e.message)?;
            eprintln!("Set the password of {name}; their sessions ended.");
        }
    }
    Ok(())
}

/// Asks `/api/health` on the bind address (127.0.0.1 for 0.0.0.0).
fn healthcheck(bind: SocketAddr) -> Result<(), String> {
    let mut addr = bind;
    if addr.ip().is_unspecified() {
        addr.set_ip(if addr.is_ipv4() {
            [127, 0, 0, 1].into()
        } else {
            std::net::Ipv6Addr::LOCALHOST.into()
        });
    }
    let mut stream =
        TcpStream::connect_timeout(&addr, Duration::from_secs(5)).map_err(|e| format!("{addr}: {e}"))?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).map_err(|e| e.to_string())?;
    stream
        .write_all(b"GET /api/health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .map_err(|e| e.to_string())?;
    let mut response = String::new();
    stream.take(4096).read_to_string(&mut response).map_err(|e| e.to_string())?;
    if response.starts_with("HTTP/1.1 200") {
        Ok(())
    } else {
        Err(format!("unhealthy: {}", response.lines().next().unwrap_or("no answer")))
    }
}
