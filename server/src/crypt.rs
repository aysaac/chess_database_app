//! Encrypted backups (NET-03): encryption at rest for the copies of the
//! server's data that leave its disk. With `CHESS_SERVER_BACKUP_KEY_FILE`
//! set, every file of a backup is written encrypted, as `<name>.enc`, and
//! the plain store copy never reaches the backup folder (it's made in the
//! data directory's `tmp/` and encrypted from there).
//!
//! - **The key**: 32 random bytes, written as 64 hex digits in a file of
//!   its own (`chess-server backup key FILE`), kept away from the backups:
//!   whoever has both reads everything. Its id (the first 8 bytes of its
//!   SHA-256, in hex) is in each backup's `ENCRYPTED` file, to tell which
//!   key it needs.
//! - **A file**: `CHSBKE01`, a random 19-byte nonce, then the content in
//!   chunks of 64 KiB, each sealed with XChaCha20-Poly1305 in the STREAM
//!   construction (a counter and a last-chunk flag in each nonce), with the
//!   file's path in the backup as associated data. A changed, cut short,
//!   reordered or swapped file fails to decrypt; nothing is written from
//!   a chunk before its tag is checked.
//! - **Restoring**: `chess-server backup decrypt --key FILE BACKUP OUT`
//!   writes the plain backup to a new folder, then it's restored as any
//!   other (`docs/server.md`, Backups).
//!
//! The live data directory isn't encrypted by this: the server reads its
//! databases at every request. Put it on an encrypted disk (LUKS, BitLocker,
//! FileVault) for that.

use std::fs::File;
use std::io::{self, ErrorKind, Read, Write};
use std::path::{Path, PathBuf};

use chacha20poly1305::aead::stream::{DecryptorBE32, EncryptorBE32};
use chacha20poly1305::aead::{KeyInit, Payload};
use chacha20poly1305::{Key, XChaCha20Poly1305};
use sha2::{Digest, Sha256};

const MAGIC: &[u8; 8] = b"CHSBKE01";
/// XChaCha20's 24-byte nonce less STREAM's 5 bytes (counter and flag).
const NONCE: usize = 19;
const CHUNK: usize = 64 * 1024;
const TAG: usize = 16;
/// The suffix of an encrypted file, and the file saying a backup is one.
pub const SUFFIX: &str = ".enc";
pub const MARKER: &str = "ENCRYPTED";

/// A backup key.
#[derive(Clone)]
pub struct BackupKey {
    key: [u8; 32],
}

impl std::fmt::Debug for BackupKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "BackupKey({})", self.id())
    }
}

fn bad(message: impl Into<String>) -> io::Error {
    io::Error::new(ErrorKind::InvalidData, message.into())
}

impl BackupKey {
    pub fn generate() -> BackupKey {
        let mut key = [0u8; 32];
        getrandom::fill(&mut key).expect("the OS random number generator works");
        BackupKey { key }
    }

    /// The key as its file holds it: 64 hex digits.
    pub fn to_text(&self) -> String {
        self.key.iter().map(|b| format!("{b:02x}")).collect()
    }

    pub fn from_text(text: &str) -> io::Result<BackupKey> {
        let text = text.trim();
        let wrong = || bad("a backup key is 64 hex digits (made by `chess-server backup key FILE`)");
        if text.len() != 64 || !text.is_ascii() {
            return Err(wrong());
        }
        let mut key = [0u8; 32];
        for (i, b) in key.iter_mut().enumerate() {
            *b = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).map_err(|_| wrong())?;
        }
        Ok(BackupKey { key })
    }

    pub fn from_file(path: &Path) -> io::Result<BackupKey> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", path.display())))?;
        BackupKey::from_text(&text).map_err(|e| bad(format!("{}: {e}", path.display())))
    }

    /// Writes a new key file at `path` (refusing to replace one), readable
    /// by its owner only where the system has such permissions.
    pub fn write_new(path: &Path) -> io::Result<BackupKey> {
        let key = BackupKey::generate();
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file =
            options.open(path).map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", path.display())))?;
        file.write_all(format!("{}\n", key.to_text()).as_bytes())?;
        file.sync_all()?;
        Ok(key)
    }

    /// Which key it is, without giving it away: the first 8 bytes of its
    /// SHA-256, in hex.
    pub fn id(&self) -> String {
        Sha256::digest(self.key)[..8].iter().map(|b| format!("{b:02x}")).collect()
    }

    fn cipher(&self) -> XChaCha20Poly1305 {
        XChaCha20Poly1305::new(Key::from_slice(&self.key))
    }
}

/// Reads up to `buf.len()` bytes, fewer only at the end.
fn fill(reader: &mut impl Read, buf: &mut [u8]) -> io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match reader.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(n)
}

/// Encrypts `from` into a new file `to`, its path in the backup (`aad`,
/// with `/` between folders) bound to it. Synced.
pub fn encrypt_file(key: &BackupKey, from: &Path, to: &Path, aad: &str) -> io::Result<()> {
    let mut input = File::open(from)?;
    let mut out = File::create(to)?;
    let mut nonce = [0u8; NONCE];
    getrandom::fill(&mut nonce).expect("the OS random number generator works");
    out.write_all(MAGIC)?;
    out.write_all(&nonce)?;
    let mut stream = EncryptorBE32::from_aead(key.cipher(), (&nonce).into());
    let seal = |e| io::Error::other(format!("encrypting {aad}: {e}"));
    // One chunk ahead, so the last one is known to be the last.
    let mut current = vec![0u8; CHUNK];
    let mut next = vec![0u8; CHUNK];
    let mut len = fill(&mut input, &mut current)?;
    loop {
        if len < CHUNK {
            let sealed =
                stream.encrypt_last(Payload { msg: &current[..len], aad: aad.as_bytes() }).map_err(seal)?;
            out.write_all(&sealed)?;
            break;
        }
        let next_len = fill(&mut input, &mut next)?;
        if next_len == 0 {
            let sealed = stream.encrypt_last(Payload { msg: &current, aad: aad.as_bytes() }).map_err(seal)?;
            out.write_all(&sealed)?;
            break;
        }
        let sealed = stream.encrypt_next(Payload { msg: &current, aad: aad.as_bytes() }).map_err(seal)?;
        out.write_all(&sealed)?;
        std::mem::swap(&mut current, &mut next);
        len = next_len;
    }
    out.sync_all()?;
    Ok(())
}

/// Decrypts `from` (made by [`encrypt_file`] for `aad`) into a new file
/// `to`; an error if it was made with another key, for another path, or
/// changed since (`to` is then removed).
pub fn decrypt_file(key: &BackupKey, from: &Path, to: &Path, aad: &str) -> io::Result<()> {
    let result = (|| {
        let mut input = File::open(from)?;
        let mut head = [0u8; 8 + NONCE];
        if fill(&mut input, &mut head)? != head.len() || &head[..8] != MAGIC {
            return Err(bad(format!("{aad}: not an encrypted backup file")));
        }
        let nonce: [u8; NONCE] = head[8..].try_into().expect("19 bytes");
        let mut stream = DecryptorBE32::from_aead(key.cipher(), (&nonce).into());
        let mut out = File::create(to)?;
        let refused = || bad(format!("{aad}: can't be decrypted: another key, or the file was changed"));
        let mut current = vec![0u8; CHUNK + TAG];
        let mut next = vec![0u8; CHUNK + TAG];
        let mut len = fill(&mut input, &mut current)?;
        loop {
            let next_len = if len == CHUNK + TAG { fill(&mut input, &mut next)? } else { 0 };
            if next_len == 0 {
                let plain = stream
                    .decrypt_last(Payload { msg: &current[..len], aad: aad.as_bytes() })
                    .map_err(|_| refused())?;
                out.write_all(&plain)?;
                break;
            }
            let plain = stream
                .decrypt_next(Payload { msg: &current[..len], aad: aad.as_bytes() })
                .map_err(|_| refused())?;
            out.write_all(&plain)?;
            std::mem::swap(&mut current, &mut next);
            len = next_len;
        }
        out.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(to);
    }
    result
}

/// The marker file's text for a backup made with `key`.
pub fn marker(key: &BackupKey) -> String {
    format!("chess-server encrypted backup, version 1\nkey {}\n", key.id())
}

/// The id of the key a backup's marker names, if the backup is encrypted.
pub fn marker_key(backup: &Path) -> Option<String> {
    let text = std::fs::read_to_string(backup.join(MARKER)).ok()?;
    text.lines().find_map(|l| l.strip_prefix("key ")).map(|k| k.trim().to_owned())
}

/// The files under `dir`, as paths relative to it, folders first.
fn files_under(dir: &Path, rel: &Path, out: &mut Vec<PathBuf>) -> io::Result<()> {
    let mut entries: Vec<_> = std::fs::read_dir(dir.join(rel))?.collect::<Result<_, _>>()?;
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let path = rel.join(e.file_name());
        if e.file_type()?.is_dir() {
            files_under(dir, &path, out)?;
        } else {
            out.push(path);
        }
    }
    Ok(())
}

/// A relative path as associated data: its parts joined by `/`, the same
/// on every system.
pub fn aad_of(rel: &Path) -> String {
    rel.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/")
}

/// Decrypts the encrypted backup `backup` into the new folder `out`, as a
/// plain backup to restore from; how many files. `out` must not exist (it's
/// written as `<out>.partial` and renamed once complete).
pub fn decrypt_backup(key: &BackupKey, backup: &Path, out: &Path) -> io::Result<u64> {
    let Some(id) = marker_key(backup) else {
        return Err(bad(format!("{} isn't an encrypted backup (it has no {MARKER} file)", backup.display())));
    };
    if id != key.id() {
        return Err(bad(format!("{} was made with key {id}; this key is {}", backup.display(), key.id())));
    }
    if out.exists() {
        return Err(io::Error::new(ErrorKind::AlreadyExists, format!("{} exists already", out.display())));
    }
    let mut partial = out.as_os_str().to_owned();
    partial.push(".partial");
    let partial = PathBuf::from(partial);
    let result = (|| {
        let mut files = Vec::new();
        files_under(backup, Path::new(""), &mut files)?;
        let mut n = 0;
        for rel in files {
            let name = rel.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let Some(plain) = name.strip_suffix(SUFFIX) else { continue };
            let to_rel = rel.with_file_name(plain);
            let to = partial.join(&to_rel);
            if let Some(parent) = to.parent() {
                std::fs::create_dir_all(parent)?;
            }
            decrypt_file(key, &backup.join(&rel), &to, &aad_of(&to_rel))?;
            n += 1;
        }
        std::fs::create_dir_all(partial.join("dbs"))?;
        Ok(n)
    })();
    match result {
        Ok(n) => {
            std::fs::rename(&partial, out)?;
            Ok(n)
        }
        Err(e) => {
            let _ = std::fs::remove_dir_all(&partial);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(len: usize) {
        let dir = tempfile::tempdir().unwrap();
        let key = BackupKey::generate();
        let data: Vec<u8> = (0..len).map(|i| (i * 7 + i / 300) as u8).collect();
        let (plain, sealed, back) = (dir.path().join("p"), dir.path().join("s"), dir.path().join("b"));
        std::fs::write(&plain, &data).unwrap();
        encrypt_file(&key, &plain, &sealed, "dbs/x/games.blob").unwrap();
        let bytes = std::fs::read(&sealed).unwrap();
        assert_eq!(bytes.len(), 8 + NONCE + len + TAG * len.div_ceil(CHUNK).max(1), "{len}");
        if len > 64 {
            assert!(!bytes.windows(64).any(|w| w == &data[..64]), "no plain text in it");
        }
        decrypt_file(&key, &sealed, &back, "dbs/x/games.blob").unwrap();
        assert_eq!(std::fs::read(&back).unwrap(), data, "{len}");
        std::fs::remove_file(&back).unwrap();
        // Another path, another key: refused, nothing left behind.
        assert!(decrypt_file(&key, &sealed, &back, "dbs/x/games.idx").is_err());
        assert!(decrypt_file(&BackupKey::generate(), &sealed, &back, "dbs/x/games.blob").is_err());
        assert!(!back.exists());
    }

    #[test]
    fn files_go_round_at_any_length() {
        for len in [0, 1, 1000, CHUNK - 1, CHUNK, CHUNK + 1, 3 * CHUNK, 3 * CHUNK + 17] {
            round_trip(len);
        }
    }

    #[test]
    fn a_changed_cut_or_lengthened_file_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let key = BackupKey::generate();
        let (plain, sealed, back) = (dir.path().join("p"), dir.path().join("s"), dir.path().join("b"));
        std::fs::write(&plain, vec![5u8; 3 * CHUNK + 10]).unwrap();
        encrypt_file(&key, &plain, &sealed, "f").unwrap();
        let good = std::fs::read(&sealed).unwrap();
        let chunk = CHUNK + TAG;
        let flipped = |at: usize| {
            let mut b = good.clone();
            b[at] ^= 1;
            b
        };
        let cases = [
            flipped(8 + NONCE + 5),
            flipped(good.len() - 1),
            flipped(10),
            good[..8 + NONCE + chunk].to_vec(),
            good[..good.len() - 1].to_vec(),
            [good.clone(), vec![0; 20]].concat(),
            [
                &good[..8 + NONCE],
                &good[8 + NONCE + chunk..8 + NONCE + 2 * chunk],
                &good[8 + NONCE..8 + NONCE + chunk],
                &good[8 + NONCE + 2 * chunk..],
            ]
            .concat(),
        ];
        for (i, bytes) in cases.iter().enumerate() {
            std::fs::write(&sealed, bytes).unwrap();
            assert!(decrypt_file(&key, &sealed, &back, "f").is_err(), "case {i}");
            assert!(!back.exists(), "case {i}");
        }
    }

    #[test]
    fn keys_are_written_read_and_named() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("backup.key");
        let key = BackupKey::write_new(&path).unwrap();
        assert!(BackupKey::write_new(&path).is_err(), "an existing key is never replaced");
        let read = BackupKey::from_file(&path).unwrap();
        assert_eq!((read.to_text(), read.id()), (key.to_text(), key.id()));
        assert_eq!(key.id().len(), 16);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        for bad in ["", "abc", &"g".repeat(64), &"a".repeat(63)] {
            assert!(BackupKey::from_text(bad).is_err(), "{bad}");
        }
        assert!(!format!("{key:?}").contains(&key.to_text()));
    }
}
