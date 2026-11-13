//! Automatic shell integration for the shells Ultty starts.
//!
//! The scripts in `dist/shell-integration` mark prompts and commands (OSC
//! 133) and report the working directory (OSC 7). This module makes a shell
//! Ultty spawns load them without the user editing an rc file, the way Ghostty
//! and Kitty do:
//!
//! - **bash** starts with `--rcfile` pointing at a wrapper that runs the real
//!   `~/.bashrc`, then the integration. Unlike `--posix` with `ENV`, this
//!   leaves bash's normal startup untouched (no posix mode to switch off, and
//!   `/etc/bash.bashrc` still runs first). A login shell (`-l`, `--login`, or
//!   a leading `-` in argv[0]) ignores `--rcfile`, so it is started as the
//!   user asked and gets no integration.
//! - **zsh** starts with `ZDOTDIR` pointing at a directory holding a
//!   `.zshenv` that restores the original `ZDOTDIR` (kept in
//!   `ULTTY_ZSH_ZDOTDIR`), runs the user's `.zshenv`, and loads the
//!   integration from the first prompt, after `.zshrc`.
//! - **fish** starts with Ultty's directory first on `XDG_DATA_DIRS`, so fish
//!   loads the script from `vendor_conf.d`; the script takes the entry out
//!   again.
//! - Any other shell, or a shell given arguments, is left alone.
//!
//! The files are embedded in the binary and written to a private directory
//! on first use, so nothing needs installing. `ULTTY_SHELL_INTEGRATION=off`
//! in the environment, or `terminal.shell_integration = "off"` in the config,
//! turns injection off. Ultty sets `ULTTY_SHELL_INTEGRATION=auto` and
//! `ULTTY_SHELL_INTEGRATION_DIR` in the shells it injects.

use crate::platform::socket::{current_uid, ensure_private_dir};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Environment variable that turns injection off (`off`, `0`, `false`, `no`)
/// and, in an injected shell, reads `auto`.
pub const ENV_MODE: &str = "ULTTY_SHELL_INTEGRATION";
/// Directory holding the integration files, set in injected shells.
pub const ENV_DIR: &str = "ULTTY_SHELL_INTEGRATION_DIR";
/// zsh: the user's original `ZDOTDIR`, present only if one was set.
const ENV_ZSH_ZDOTDIR: &str = "ULTTY_ZSH_ZDOTDIR";
/// fish: the `XDG_DATA_DIRS` entry the script removes again.
const ENV_FISH_XDG_DIR: &str = "ULTTY_SHELL_INTEGRATION_XDG_DIR";

/// Files written under the integration directory, by relative path.
const FILES: &[(&str, &str)] = &[
    (
        "bash/ultty.bash",
        include_str!("../../../../dist/shell-integration/ultty.bash"),
    ),
    (
        "bash/ultty.bashrc",
        include_str!("../../../../dist/shell-integration/inject/bashrc"),
    ),
    (
        "zsh/ultty.zsh",
        include_str!("../../../../dist/shell-integration/ultty.zsh"),
    ),
    (
        "zsh/.zshenv",
        include_str!("../../../../dist/shell-integration/inject/zshenv"),
    ),
    (
        "fish/vendor_conf.d/ultty.fish",
        include_str!("../../../../dist/shell-integration/ultty.fish"),
    ),
];

/// How to start a shell so it loads the integration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Injection {
    /// The complete argv, argv[0] included.
    pub args: Vec<String>,
    /// Variables to add to the shell's environment.
    pub env: Vec<(OsString, OsString)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shell {
    Bash,
    Zsh,
    Fish,
}

fn detect(program: &str) -> Option<Shell> {
    let name = Path::new(program).file_name()?.to_str()?;
    match name.strip_prefix('-').unwrap_or(name) {
        "bash" => Some(Shell::Bash),
        "zsh" => Some(Shell::Zsh),
        "fish" => Some(Shell::Fish),
        _ => None,
    }
}

/// Whether `value` of [`ENV_MODE`] asks for injection to be off.
pub fn mode_is_off(value: &OsStr) -> bool {
    matches!(
        value.to_str().map(str::to_ascii_lowercase).as_deref(),
        Some("off" | "0" | "false" | "no")
    )
}

/// The injection for starting `program` with `args` (argv, argv[0] first)
/// when the integration files are in `dir`, or `None` to start it unchanged.
///
/// `var` looks a variable up in the environment the shell will have.
pub fn plan(
    program: &str,
    args: &[&str],
    var: &dyn Fn(&str) -> Option<OsString>,
    dir: &Path,
) -> Option<Injection> {
    let shell = detect(program)?;
    let dir_str = dir.to_str()?;
    // Only a bare interactive start: anything else is the caller's choice.
    let argv0 = args.first().copied().unwrap_or(program);
    if args.len() > 1 || argv0.starts_with('-') {
        return None;
    }
    let mut env: Vec<(OsString, OsString)> = vec![
        (ENV_MODE.into(), "auto".into()),
        (ENV_DIR.into(), dir.as_os_str().to_owned()),
    ];
    let args = match shell {
        Shell::Bash => vec![
            argv0.to_string(),
            "--rcfile".to_string(),
            format!("{dir_str}/bash/ultty.bashrc"),
        ],
        Shell::Zsh => {
            if let Some(original) = var("ZDOTDIR") {
                env.push((ENV_ZSH_ZDOTDIR.into(), original));
            }
            env.push(("ZDOTDIR".into(), format!("{dir_str}/zsh").into()));
            vec![argv0.to_string()]
        }
        Shell::Fish => {
            let mut dirs = OsString::from(dir_str);
            dirs.push(":");
            dirs.push(
                var("XDG_DATA_DIRS")
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| "/usr/local/share:/usr/share".into()),
            );
            env.push(("XDG_DATA_DIRS".into(), dirs));
            env.push((ENV_FISH_XDG_DIR.into(), dir.as_os_str().to_owned()));
            vec![argv0.to_string()]
        }
    };
    Some(Injection { args, env })
}

/// [`plan`] for a shell about to be spawned with `env` added to this
/// process's environment: honours the opt-out and writes the files on first
/// use. `None` means start the shell unchanged.
pub fn for_spawn(program: &str, args: &[&str], env: &[(OsString, OsString)]) -> Option<Injection> {
    detect(program)?;
    let var = |name: &str| -> Option<OsString> {
        env.iter()
            .rev()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.clone())
            .or_else(|| std::env::var_os(name))
    };
    if var(ENV_MODE).is_some_and(|value| mode_is_off(&value)) {
        return None;
    }
    plan(program, args, &var, runtime_dir()?)
}

/// The directory the embedded files are written to, once per process.
fn runtime_dir() -> Option<&'static Path> {
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| match private_base().and_then(|base| install(&base)) {
        Ok(dir) => Some(dir),
        Err(error) => {
            tracing::warn!("shell integration unavailable: {error}");
            None
        }
    })
    .as_deref()
}

/// A directory only this user can write: `$XDG_RUNTIME_DIR` if it passes the
/// ownership and mode check, else `<temp>/ultty-<uid>`.
fn private_base() -> io::Result<PathBuf> {
    if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from) {
        if runtime.is_absolute() && ensure_private_dir(&runtime).is_ok() {
            return Ok(runtime);
        }
    }
    let dir = std::env::temp_dir().join(format!("ultty-{}", current_uid()));
    ensure_private_dir(&dir)?;
    Ok(dir)
}

/// Write the embedded files under `base` and return the directory holding
/// them, which is named for their content so versions never mix.
///
/// `base` must already be private to this user. The files are written to a
/// fresh 0700 directory and moved into place with one rename, so a reader
/// never sees a half-written set. If the final directory exists already (an
/// earlier run, or another Ultty racing this one) it is used only if it is a
/// 0700 directory of ours with exactly the expected content.
pub fn install(base: &Path) -> io::Result<PathBuf> {
    let name = format!(
        "ultty-shell-integration-{}-{:016x}",
        env!("CARGO_PKG_VERSION"),
        content_hash()
    );
    let target = base.join(&name);
    if fs::symlink_metadata(&target).is_ok() {
        return verify(&target);
    }
    let staging = base.join(format!(".{name}.{}", std::process::id()));
    // A stale directory from a process that died with this pid is ours to clear.
    if fs::symlink_metadata(&staging).is_ok() {
        ensure_private_dir(&staging)?;
        fs::remove_dir_all(&staging)?;
    }
    let result = stage(&staging).and_then(|()| match fs::rename(&staging, &target) {
        Ok(()) => Ok(target.clone()),
        // Lost a race: the other copy is checked like any existing one.
        Err(_) if fs::symlink_metadata(&target).is_ok() => verify(&target),
        Err(error) => Err(error),
    });
    if staging.exists() {
        let _ = fs::remove_dir_all(&staging);
    }
    result
}

fn stage(staging: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.mode(0o700);
    builder.create(staging)?;
    for (relative, content) in FILES {
        let path = staging.join(relative);
        if let Some(parent) = path.parent() {
            builder.recursive(true).create(parent)?;
        }
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&path)?;
        file.write_all(content.as_bytes())?;
    }
    Ok(())
}

fn verify(dir: &Path) -> io::Result<PathBuf> {
    ensure_private_dir(dir)?;
    for (relative, content) in FILES {
        let path = dir.join(relative);
        let mismatch = || {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "{} is not the expected shell integration file",
                    path.display()
                ),
            )
        };
        let meta = fs::symlink_metadata(&path)?;
        if !meta.file_type().is_file() {
            return Err(mismatch());
        }
        if fs::read(&path)? != content.as_bytes() {
            return Err(mismatch());
        }
    }
    Ok(dir.to_path_buf())
}

/// FNV-1a over the embedded files: a name for this exact content.
fn content_hash() -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for (relative, content) in FILES {
        for byte in relative
            .bytes()
            .chain([0])
            .chain(content.bytes())
            .chain([0])
        {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    const DIR: &str = "/run/user/1/ultty-shell-integration-x";

    fn no_env(_: &str) -> Option<OsString> {
        None
    }

    fn plan_with(program: &str, args: &[&str], vars: &[(&str, &str)]) -> Option<Injection> {
        let lookup = |name: &str| {
            vars.iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| OsString::from(*value))
        };
        plan(program, args, &lookup, Path::new(DIR))
    }

    fn env_of(injection: &Injection, name: &str) -> Option<String> {
        injection
            .env
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.to_string_lossy().into_owned())
    }

    #[test]
    fn bash_gets_an_rcfile_wrapper() {
        let injection = plan_with("/usr/bin/bash", &["/usr/bin/bash"], &[]).unwrap();
        assert_eq!(
            injection.args,
            [
                "/usr/bin/bash",
                "--rcfile",
                format!("{DIR}/bash/ultty.bashrc").as_str()
            ]
        );
        assert_eq!(env_of(&injection, ENV_MODE).unwrap(), "auto");
        assert_eq!(env_of(&injection, ENV_DIR).unwrap(), DIR);
        assert!(env_of(&injection, "ZDOTDIR").is_none());
    }

    #[test]
    fn login_bash_is_started_as_asked() {
        assert_eq!(plan_with("/bin/bash", &["/bin/bash", "-l"], &[]), None);
        assert_eq!(plan_with("/bin/bash", &["/bin/bash", "--login"], &[]), None);
        assert_eq!(plan_with("/bin/bash", &["-bash"], &[]), None);
        assert_eq!(plan_with("-bash", &["-bash"], &[]), None);
    }

    #[test]
    fn a_shell_with_arguments_is_left_alone() {
        for shell in ["bash", "zsh", "fish"] {
            let program = format!("/bin/{shell}");
            assert_eq!(
                plan_with(&program, &[&program, "-c", "true"], &[]),
                None,
                "{shell}"
            );
        }
    }

    #[test]
    fn zsh_points_zdotdir_at_ours_and_keeps_the_original() {
        let injection =
            plan_with("/bin/zsh", &["/bin/zsh"], &[("ZDOTDIR", "/home/u/zdot")]).unwrap();
        assert_eq!(injection.args, ["/bin/zsh"]);
        assert_eq!(env_of(&injection, "ZDOTDIR").unwrap(), format!("{DIR}/zsh"));
        assert_eq!(env_of(&injection, ENV_ZSH_ZDOTDIR).unwrap(), "/home/u/zdot");
    }

    #[test]
    fn zsh_without_a_zdotdir_records_none() {
        let injection = plan_with("/bin/zsh", &["/bin/zsh"], &[]).unwrap();
        assert_eq!(env_of(&injection, "ZDOTDIR").unwrap(), format!("{DIR}/zsh"));
        assert!(env_of(&injection, ENV_ZSH_ZDOTDIR).is_none());
    }

    #[test]
    fn zsh_keeps_an_empty_original_zdotdir() {
        let injection = plan_with("/bin/zsh", &["/bin/zsh"], &[("ZDOTDIR", "")]).unwrap();
        assert_eq!(env_of(&injection, ENV_ZSH_ZDOTDIR).unwrap(), "");
    }

    #[test]
    fn fish_prepends_its_directory_to_xdg_data_dirs() {
        let injection = plan_with(
            "/usr/bin/fish",
            &["/usr/bin/fish"],
            &[("XDG_DATA_DIRS", "/a:/b")],
        )
        .unwrap();
        assert_eq!(injection.args, ["/usr/bin/fish"]);
        assert_eq!(
            env_of(&injection, "XDG_DATA_DIRS").unwrap(),
            format!("{DIR}:/a:/b")
        );
        assert_eq!(env_of(&injection, ENV_FISH_XDG_DIR).unwrap(), DIR);

        let default = plan_with("/usr/bin/fish", &["/usr/bin/fish"], &[]).unwrap();
        assert_eq!(
            env_of(&default, "XDG_DATA_DIRS").unwrap(),
            format!("{DIR}:/usr/local/share:/usr/share")
        );
    }

    #[test]
    fn other_shells_are_left_alone() {
        for shell in [
            "/bin/sh",
            "/usr/bin/nu",
            "/usr/bin/elvish",
            "/bin/dash",
            "/bin/tcsh",
        ] {
            assert_eq!(plan_with(shell, &[shell], &[]), None, "{shell}");
        }
        assert!(plan("", &[], &no_env, Path::new(DIR)).is_none());
    }

    #[test]
    fn the_off_switch_is_read_from_the_spawn_env_and_the_process_env() {
        for value in ["off", "OFF", "0", "false", "no"] {
            assert!(mode_is_off(OsStr::new(value)), "{value}");
        }
        for value in ["auto", "", "1", "on"] {
            assert!(!mode_is_off(OsStr::new(value)), "{value}");
        }
        let off = [(OsString::from(ENV_MODE), OsString::from("off"))];
        assert_eq!(for_spawn("/bin/bash", &["/bin/bash"], &off), None);
        assert_eq!(for_spawn("/bin/zsh", &["/bin/zsh"], &off), None);
    }

    #[test]
    fn install_writes_a_private_tree_and_is_repeatable() {
        let base = tempfile::tempdir().unwrap();
        fs::set_permissions(base.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let dir = install(base.path()).unwrap();
        let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&dir), 0o700);
        assert_eq!(mode(&dir.join("zsh")), 0o700);
        for (relative, content) in FILES {
            let path = dir.join(relative);
            assert_eq!(fs::read_to_string(&path).unwrap(), *content, "{relative}");
            assert_eq!(mode(&path), 0o600, "{relative}");
        }
        assert_eq!(install(base.path()).unwrap(), dir);
        let leftovers: Vec<_> = fs::read_dir(base.path()).unwrap().collect();
        assert_eq!(leftovers.len(), 1, "no staging directory is left behind");
    }

    #[test]
    fn install_refuses_a_directory_it_does_not_trust() {
        let base = tempfile::tempdir().unwrap();
        fs::set_permissions(base.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let dir = install(base.path()).unwrap();

        fs::write(dir.join("zsh/.zshenv"), "echo pwned\n").unwrap();
        assert!(install(base.path()).is_err(), "tampered content");
        fs::write(dir.join("zsh/.zshenv"), FILES[3].1).unwrap();

        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(install(base.path()).is_err(), "group- or world-readable");
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(install(base.path()).unwrap(), dir);

        let elsewhere = base.path().join("elsewhere");
        let moved = base.path().join("moved");
        fs::rename(&dir, &elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &dir).unwrap();
        assert!(install(base.path()).is_err(), "symlinked directory");
        let _ = fs::remove_file(&dir);
        let _ = fs::rename(&elsewhere, &moved);
    }
}
