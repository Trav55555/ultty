//! `--exec` grants for Luau tools (ADR 0024 §3).
//!
//! `ulttui-lua --exec CMD` lets a script's `run{}` start command lines
//! beginning with the words of `CMD`. The grant is checked in two places:
//! `ulttui-lua` at startup, and `ulttyctl lua open` before it asks Ultty for
//! a pane, so a refused grant is reported to the person who typed it instead
//! of in a pane that closes. Both use [`parse`], so they cannot disagree.

use std::path::Path;

/// Programs whose arguments are a program to run: a grant starting with one
/// of these (by basename) is `--allow-exec` by another name, so [`parse`]
/// refuses it and asks for the explicit flag. A guard against mistakes, not
/// a sandbox: `git` with no subcommand runs hooks, aliases and pagers and is
/// still allowed. Listed in `docs/lua.md`. Shells first, then wrappers that
/// exec their arguments, then interpreters.
pub const PROGRAM_RUNNERS: &[&str] = &[
    "sh", "bash", "zsh", "dash", "fish", "ksh", "csh", "tcsh", "env", "xargs", "exec", "nohup",
    "setsid", "sudo", "doas", "su", "timeout", "nice", "ionice", "stdbuf", "flock", "watch",
    "script", "ssh", "python", "python3", "perl", "ruby", "node", "deno", "bun", "lua", "luajit",
    "php", "tclsh", "awk", "gawk", "busybox",
];

/// Programs that are program runners only without a subcommand word:
/// `ulttyctl` alone reaches `new-tab`, `split` and `lua open --allow-exec`;
/// `'ulttyctl --json panes'` is one fixed command.
pub const NEED_SUBCOMMAND: &[&str] = &["ulttyctl", "ulttui-lua"];

/// The words of one `--exec` grant: split on whitespace, with single or
/// double quotes keeping an argument together. A grant that would allow
/// anything (see [`PROGRAM_RUNNERS`]) is an error naming `--allow-exec`.
pub fn parse(spec: &str) -> Result<Vec<String>, String> {
    let words = split_words(spec)?;
    let program = Path::new(&words[0])
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    if PROGRAM_RUNNERS.contains(&program.as_str()) {
        return Err(format!(
            "--exec {spec:?}: {program} runs whatever it is told to, so this grant would allow anything; say so with --allow-exec"
        ));
    }
    if words.len() == 1 && NEED_SUBCOMMAND.contains(&program.as_str()) {
        return Err(format!(
            "--exec {spec:?}: a bare {program} can open panes running anything; grant one command such as --exec '{program} --json panes', or allow anything with --allow-exec"
        ));
    }
    Ok(words)
}

fn split_words(spec: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut word: Option<String> = None;
    let mut quote: Option<char> = None;
    for c in spec.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => word.get_or_insert_with(String::new).push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                word.get_or_insert_with(String::new);
            }
            (None, c) if c.is_whitespace() => words.extend(word.take()),
            (None, c) => word.get_or_insert_with(String::new).push(c),
        }
    }
    if quote.is_some() {
        return Err(format!("--exec {spec:?}: unbalanced quote"));
    }
    words.extend(word);
    if words.is_empty() {
        return Err("--exec needs a program".to_string());
    }
    Ok(words)
}
