//! Embeds the admin web UI (`admin/dist`, built by `npm run build` in
//! `admin/`) in the binary, so the server makes no network fetch for it.
//! Without a build, a page saying how to make one is embedded instead, so
//! `cargo test --workspace` works on a fresh checkout.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

fn files(dir: &Path, rel: &str, out: &mut Vec<(String, PathBuf)>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = rd.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name = e.file_name().to_string_lossy().into_owned();
        let path = e.path();
        let rel = if rel.is_empty() { name } else { format!("{rel}/{name}") };
        if path.is_dir() {
            files(&path, &rel, out);
        } else {
            out.push((rel, path));
        }
    }
}

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"));
    let dist = manifest.join("admin").join("dist");
    let built = dist.join("index.html").is_file();
    // Watching a folder that doesn't exist would rerun this on every build:
    // until there's a build, watch the UI's folder for one to appear.
    if built {
        println!("cargo:rerun-if-changed=admin/dist");
    } else {
        println!("cargo:rerun-if-changed=admin");
        if std::env::var("PROFILE").as_deref() == Ok("release") {
            println!("cargo:warning=server/admin/dist is missing: the admin UI isn't built into this binary");
        }
    }

    let mut code =
        String::from("/// The admin UI's files: (path, bytes).\npub static ASSETS: &[(&str, &[u8])] = &[\n");
    if built {
        let mut list = Vec::new();
        files(&dist, "", &mut list);
        for (rel, path) in list {
            writeln!(code, "    ({rel:?}, include_bytes!({:?})),", path.display().to_string())
                .expect("write to a String");
        }
    }
    code.push_str("];\n");
    writeln!(code, "/// Whether `admin/dist` was there.\npub const BUILT: bool = {built};")
        .expect("write to a String");
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("cargo sets OUT_DIR")).join("admin_assets.rs");
    std::fs::write(out, code).expect("write the generated file");
}
