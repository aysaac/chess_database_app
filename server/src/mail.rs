//! E-mail: the password reset link of "Forgot password" (NET-03), through
//! the SMTP server in the settings (`CHESS_SERVER_SMTP_*`), with `lettre`
//! over rustls. Blocking: call it from the blocking pool.

use std::time::Duration;

use lettre::message::Mailbox;
use lettre::message::header::ContentType;
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::client::{Tls, TlsParameters};
use lettre::{Message, SmtpTransport, Transport};

use crate::config::{SmtpConfig, SmtpTls};

fn transport(smtp: &SmtpConfig) -> Result<SmtpTransport, String> {
    let tls = || TlsParameters::new(smtp.host.clone()).map_err(|e| format!("TLS for {}: {e}", smtp.host));
    let mut builder = SmtpTransport::builder_dangerous(&smtp.host)
        .port(smtp.port)
        .timeout(Some(Duration::from_secs(20)))
        .tls(match smtp.tls {
            SmtpTls::StartTls => Tls::Required(tls()?),
            SmtpTls::Tls => Tls::Wrapper(tls()?),
            SmtpTls::None => Tls::None,
        });
    if let (Some(user), Some(password)) = (&smtp.username, &smtp.password) {
        builder = builder.credentials(Credentials::new(user.clone(), password.clone()));
    }
    Ok(builder.build())
}

/// Sends `username` the link that sets a new password, at `to`.
pub fn send_reset_link(
    smtp: &SmtpConfig,
    to: &str,
    username: &str,
    link: &str,
    minutes: u64,
) -> Result<(), String> {
    let from: Mailbox = smtp.from.parse().map_err(|e| format!("CHESS_SERVER_SMTP_FROM: {e}"))?;
    let to: Mailbox = to.parse().map_err(|e| format!("the address: {e}"))?;
    let body = format!(
        "Someone asked to set a new password for the account {username} on the chess server.\n\n\
         To set one, open this link within {minutes} minutes:\n\n{link}\n\n\
         If it wasn't you, ignore this message: your password stays as it is.\n"
    );
    let message = Message::builder()
        .from(from)
        .to(to)
        .subject("Set a new password for the chess server")
        .header(ContentType::TEXT_PLAIN)
        .body(body)
        .map_err(|e| e.to_string())?;
    transport(smtp)?.send(&message).map_err(|e| e.to_string())?;
    Ok(())
}

/// Whether `s` looks like an e-mail address: one `@`, something either
/// side, a dot in the domain, no spaces or control characters, at most
/// 254 characters.
pub fn plausible_address(s: &str) -> bool {
    let Some((local, domain)) = s.split_once('@') else { return false };
    s.len() <= 254
        && !local.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !domain.contains('@')
        && !s.chars().any(|c| c.is_whitespace() || c.is_control() || c == '<' || c == '>' || c == ',')
        && s.parse::<Mailbox>().is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses() {
        for good in ["a@b.co", "first.last+tag@example.org"] {
            assert!(plausible_address(good), "{good}");
        }
        for bad in ["", "a", "a@b", "@b.co", "a@.co", "a b@c.de", "a@b.co, c@d.ef", "<a@b.co>", "a@b@c.de"] {
            assert!(!plausible_address(bad), "{bad}");
        }
    }
}
