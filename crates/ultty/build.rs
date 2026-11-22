//! Lists the bundled theme files (`themes/*.toml`) for `src/themes.rs`, so
//! adding a theme is a matter of dropping in a file (or rerunning
//! `scripts/import-themes.py`).

use std::fmt::Write as _;
use std::path::PathBuf;

fn main() {
    let dir =
        PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("manifest dir")).join("themes");
    println!("cargo:rerun-if-changed=themes");
    println!("cargo:rerun-if-changed=build.rs");

    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("themes directory")
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "toml"))
        .collect();
    files.sort();

    let mut source = String::from("static BUNDLED: &[(&str, &str)] = &[\n");
    for path in files {
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .expect("utf-8 name");
        let file_name = path
            .file_name()
            .and_then(|s| s.to_str())
            .expect("utf-8 file name");
        writeln!(
            source,
            "    ({stem:?}, include_str!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/themes/\", {file_name:?}))),"
        )
        .expect("write to string");
    }
    source.push_str("];\n");

    let out =
        PathBuf::from(std::env::var_os("OUT_DIR").expect("out dir")).join("bundled_themes.rs");
    std::fs::write(out, source).expect("write bundled_themes.rs");
}
