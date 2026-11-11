//! Random ids and tokens, token digests, and password hashing.
//!
//! Session and API tokens are 32 random bytes; the metadata store keeps
//! only their SHA-256, so a copy of it lets nobody log in. Passwords are
//! hashed with Argon2id at its default parameters (19 MiB, 2 passes), as
//! OWASP recommends.

use argon2::Argon2;
use argon2::password_hash::{PasswordHasher, PasswordVerifier};
use sha2::{Digest, Sha256};

/// Prefix of API tokens, so they're recognizable in a config file or a leak scan.
pub const TOKEN_PREFIX: &str = "cdb_";

pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut buf = [0u8; N];
    getrandom::fill(&mut buf).expect("the OS random number generator works");
    buf
}

pub fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(DIGITS[usize::from(b >> 4)] as char);
        out.push(DIGITS[usize::from(b & 15)] as char);
    }
    out
}

/// A new session, setup or CSRF token: 64 hex digits.
pub fn new_token() -> String {
    hex(&random_bytes::<32>())
}

/// A new API token: [`TOKEN_PREFIX`] and 64 hex digits.
pub fn new_api_token() -> String {
    format!("{TOKEN_PREFIX}{}", new_token())
}

/// What the metadata store keeps of a token.
pub fn token_digest(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

/// A new database id: 32 hex digits, the name of its folder.
pub fn new_db_id() -> String {
    hex(&random_bytes::<16>())
}

/// Whether `s` is a database id as [`new_db_id`] makes them. Anything else
/// in a URL (`..`, slashes, other lengths) is refused before a path is built.
pub fn is_db_id(s: &str) -> bool {
    s.len() == 32 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Compares in time independent of where the inputs differ.
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

pub fn hash_password(password: &str) -> Result<String, String> {
    Argon2::default().hash_password(password.as_bytes()).map(|h| h.to_string()).map_err(|e| e.to_string())
}

pub fn verify_password(password: &str, hash: &str) -> bool {
    Argon2::default().verify_password(password.as_bytes(), hash).is_ok()
}

/// Spends the time a check against a real hash takes, for logins with an
/// unknown username: answering faster would tell which names exist.
pub fn verify_nothing(password: &str) {
    static DUMMY: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    let hash = DUMMY.get_or_init(|| hash_password("not a password").unwrap_or_default());
    let _ = verify_password(password, hash);
}

/// Checks a new password: 8 to 1024 characters.
pub fn check_new_password(password: &str) -> Result<(), String> {
    let n = password.chars().count();
    if n < 8 {
        return Err("A password needs at least 8 characters".into());
    }
    if n > 1024 {
        return Err("A password can have at most 1024 characters".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_and_tokens() {
        let id = new_db_id();
        assert!(is_db_id(&id), "{id}");
        assert_ne!(id, new_db_id());
        for bad in [
            "",
            "..",
            "../../etc/passwd",
            &id.to_uppercase(),
            &id[1..],
            &format!("{id}0"),
            "g".repeat(32).as_str(),
        ] {
            assert!(!is_db_id(bad), "{bad}");
        }
        assert!(new_api_token().starts_with(TOKEN_PREFIX));
        assert_eq!(token_digest("a").len(), 32);
        assert_eq!(hex(&[0, 0xab, 0xff]), "00abff");
    }

    #[test]
    fn constant_time_eq() {
        assert!(ct_eq(b"abc", b"abc"));
        assert!(!ct_eq(b"abc", b"abd"));
        assert!(!ct_eq(b"abc", b"ab"));
    }

    #[test]
    fn passwords() {
        let h = hash_password("correct horse").unwrap();
        assert!(h.starts_with("$argon2id$"));
        assert!(verify_password("correct horse", &h));
        assert!(!verify_password("wrong horse", &h));
        assert!(!verify_password("x", "not a hash"));
        assert!(check_new_password("short").is_err());
        assert!(check_new_password("long enough").is_ok());
    }
}
