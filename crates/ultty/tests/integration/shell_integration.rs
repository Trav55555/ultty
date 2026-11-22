//! Real bash and zsh, started the way Ultty starts them, in a throwaway HOME.
//!
//! Each shell must still run the user's rc files, send OSC 133 and OSC 7,
//! leave `ZDOTDIR` as the user had it, and mark only once even when the
//! integration is sourced a second time. A shell that is not installed is
//! skipped.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use ultty::pty::shell_integration::{install, plan, Injection};
use ultty::pty::{Child, PtySize};

/// Where a shell finds `name`, if it is installed.
fn find(name: &str) -> Option<String> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
        .map(|candidate| candidate.to_string_lossy().into_owned())
}

struct Run {
    /// Everything the shell wrote to the terminal.
    output: String,
}

/// Start `shell` with its injection in `home`, type `lines`, and read until
/// the shell exits. `inherited` are variables the shell is given besides the
/// injected ones.
fn run_shell(
    shell: &str,
    home: &Path,
    runtime: &Path,
    inherited: &[(&str, &str)],
    flags: &[&str],
    lines: &[&str],
) -> Run {
    let dir = install(runtime).expect("install integration files");
    let lookup = |name: &str| {
        inherited
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| OsString::from(*value))
    };
    let Injection {
        args,
        env: injected,
    } = plan(shell, &argv0_and(shell, flags), &lookup, &dir).expect("shell gets injected");
    let mut env: Vec<(OsString, OsString)> = vec![
        ("HOME".into(), home.into()),
        ("TERM".into(), "xterm-256color".into()),
        // Keep the host's own shell setup out of the run.
        ("PROMPT_COMMAND".into(), "".into()),
        ("BASH_ENV".into(), "".into()),
        ("ENV".into(), "".into()),
        ("PS1".into(), "$ ".into()),
        ("XDG_CONFIG_HOME".into(), home.join(".config").into()),
    ];
    for (key, value) in inherited {
        env.push(((*key).into(), (*value).into()));
    }
    env.retain(|(key, _)| !injected.iter().any(|(added, _)| added == key));
    env.extend(injected);
    let argv: Vec<&str> = args.iter().map(String::as_str).collect();
    let mut child = Child::spawn(shell, &argv, home, PtySize::default(), &env).expect("spawn");

    let mut output = Vec::new();
    let mut buf = [0u8; 4096];
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut next_line = 0;
    let mut last_output = Instant::now();
    while Instant::now() < deadline {
        // Type the next line once the previous one has been echoed back
        // and the shell has gone quiet.
        if next_line < lines.len() && last_output.elapsed() > Duration::from_millis(400) {
            let line = format!("{}\n", lines[next_line]);
            child.write_all(line.as_bytes()).expect("write");
            next_line += 1;
            last_output = Instant::now();
        }
        match child.read(&mut buf) {
            Ok(0) => std::thread::sleep(Duration::from_millis(20)),
            Ok(n) => {
                output.extend_from_slice(&buf[..n]);
                last_output = Instant::now();
            }
            Err(_) => break,
        }
        if child.has_exited() && next_line == lines.len() {
            break;
        }
    }
    let _ = child.wait();
    Run {
        output: String::from_utf8_lossy(&output).into_owned(),
    }
}

fn argv0_and<'a>(shell: &'a str, flags: &[&'a str]) -> Vec<&'a str> {
    let mut argv = vec![shell];
    argv.extend(flags);
    argv
}

fn count(haystack: &str, needle: &str) -> usize {
    haystack.matches(needle).count()
}

struct Sandbox {
    _root: tempfile::TempDir,
    home: PathBuf,
    runtime: PathBuf,
}

fn sandbox() -> Sandbox {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().expect("tempdir");
    let home = root.path().join("home with space");
    let runtime = root.path().join("runtime");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir(&runtime).unwrap();
    std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o700)).unwrap();
    Sandbox {
        home,
        runtime,
        _root: root,
    }
}

/// The checks every shell shares. `rc_marker` is the sentinel the user's rc
/// printed, `echoed` what the shell reported about its own state.
fn assert_integrated(run: &Run, rc_marker: &str) {
    let out = &run.output;
    assert!(out.contains(rc_marker), "user rc did not run:\n{out:?}");
    assert!(
        out.contains("USER_VAR=from-rc"),
        "rc variable missing:\n{out:?}"
    );
    assert!(out.contains("\x1b]133;A"), "no OSC 133 A:\n{out:?}");
    assert!(out.contains("\x1b]133;B"), "no OSC 133 B:\n{out:?}");
    assert!(out.contains("\x1b]133;C"), "no OSC 133 C:\n{out:?}");
    assert!(out.contains("\x1b]133;D;0"), "no OSC 133 D:\n{out:?}");
    assert!(out.contains("\x1b]7;file://"), "no OSC 7:\n{out:?}");
    assert!(
        out.contains("home%20with%20space"),
        "OSC 7 path not percent-encoded:\n{out:?}"
    );
}

#[test]
fn bash_runs_the_users_bashrc_and_marks_once() {
    let Some(bash) = find("bash") else {
        eprintln!("skipped: bash is not installed");
        return;
    };
    let s = sandbox();
    std::fs::write(
        s.home.join(".bashrc"),
        "export USER_VAR=from-rc\necho RC_BASH_RAN\n",
    )
    .unwrap();
    let run = run_shell(
        &bash,
        &s.home,
        &s.runtime,
        &[],
        &[],
        &[
            "echo \"USER_VAR=$USER_VAR\"",
            "source \"$ULTTY_SHELL_INTEGRATION_DIR/bash/ultty.bash\"",
            "true",
            "exit",
        ],
    );
    assert_integrated(&run, "RC_BASH_RAN");
    // The second source must not add a second set of hooks: one prompt
    // mark per prompt, so A never appears twice in a row without a prompt.
    assert!(
        !run.output.contains("\x1b]133;A\x07\x1b]133;A"),
        "marks doubled:\n{:?}",
        run.output
    );
}

#[test]
fn zsh_keeps_the_original_zdotdir_and_runs_the_users_rc_files() {
    let Some(zsh) = find("zsh") else {
        eprintln!("skipped: zsh is not installed");
        return;
    };
    let s = sandbox();
    let zdot = s.home.join("zdot");
    std::fs::create_dir(&zdot).unwrap();
    std::fs::write(zdot.join(".zshenv"), "export ENV_FILE_RAN=yes\n").unwrap();
    std::fs::write(
        zdot.join(".zshrc"),
        "export USER_VAR=from-rc\nPS1='%# '\necho RC_ZSH_RAN\n",
    )
    .unwrap();
    let zdot_str = zdot.to_str().unwrap();
    let run = run_shell(
        &zsh,
        &s.home,
        &s.runtime,
        &[("ZDOTDIR", zdot_str)],
        &[],
        &[
            "echo \"USER_VAR=$USER_VAR ENVFILE=$ENV_FILE_RAN\"",
            "echo \"ZDOTDIR_IS=[$ZDOTDIR] SAVED=[${ULTTY_ZSH_ZDOTDIR-unset}]\"",
            "source \"$ULTTY_SHELL_INTEGRATION_DIR/zsh/ultty.zsh\"",
            "true",
            "exit",
        ],
    );
    assert_integrated(&run, "RC_ZSH_RAN");
    assert!(
        run.output.contains("ENVFILE=yes"),
        "user .zshenv did not run:\n{:?}",
        run.output
    );
    assert!(
        run.output
            .contains(&format!("ZDOTDIR_IS=[{zdot_str}] SAVED=[unset]")),
        "ZDOTDIR not restored:\n{:?}",
        run.output
    );
    // Five lines were typed, the last of them `exit`. Hooks loaded twice
    // would send C and the prompt marks twice per line. (zsh's end-of-line
    // mark may add a second, harmless D per command; Ultty ignores a D with
    // no command running.)
    assert_eq!(count(&run.output, "\x1b]133;C"), 5, "{:?}", run.output);
    assert_eq!(
        count(&run.output, "\x1b]133;A"),
        count(&run.output, "\x1b]133;B"),
        "{:?}",
        run.output
    );
}

#[test]
fn zsh_without_a_zdotdir_reads_dotfiles_from_home_and_leaves_it_unset() {
    let Some(zsh) = find("zsh") else {
        eprintln!("skipped: zsh is not installed");
        return;
    };
    if std::env::var_os("ZDOTDIR").is_some() {
        eprintln!("skipped: ZDOTDIR is set in the test environment");
        return;
    }
    let s = sandbox();
    std::fs::write(
        s.home.join(".zshrc"),
        "export USER_VAR=from-rc\nPS1='%# '\necho RC_ZSH_RAN\n",
    )
    .unwrap();
    let run = run_shell(
        &zsh,
        &s.home,
        &s.runtime,
        &[],
        &[],
        &[
            "echo \"USER_VAR=$USER_VAR\"",
            "echo \"ZDOTDIR_IS=[${ZDOTDIR-unset}]\"",
            "exit",
        ],
    );
    assert_integrated(&run, "RC_ZSH_RAN");
    assert!(
        run.output.contains("ZDOTDIR_IS=[unset]"),
        "ZDOTDIR not left unset:\n{:?}",
        run.output
    );
}

#[test]
fn login_bash_runs_the_login_files_and_leaves_posix_mode() {
    let Some(bash) = find("bash") else {
        eprintln!("skipped: bash is not installed");
        return;
    };
    let s = sandbox();
    // Only .bash_profile may run: .bash_login and .profile come after it,
    // and .bashrc runs only because the profile sources it.
    std::fs::write(
        s.home.join(".bash_profile"),
        "echo PROFILE_RAN\n[ -r \"$HOME/.bashrc\" ] && . \"$HOME/.bashrc\"\n",
    )
    .unwrap();
    std::fs::write(s.home.join(".bash_login"), "echo WRONG_LOGIN_FILE\n").unwrap();
    std::fs::write(s.home.join(".profile"), "echo WRONG_PROFILE\n").unwrap();
    std::fs::write(
        s.home.join(".bashrc"),
        "export USER_VAR=from-rc\necho RC_BASH_RAN\n",
    )
    .unwrap();
    for flag in [&["-l"][..], &["--login"]] {
        let run = run_shell(
            &bash,
            &s.home,
            &s.runtime,
            &[("ENV", "/nonexistent/user-env")],
            flag,
            &[
                "echo \"USER_VAR=$USER_VAR\"",
                "shopt -q login_shell && echo LOGIN_SHELL_YES",
                "set -o | grep -q '^posix.*on' && echo POSIX_ON || echo POSIX_OFF",
                "echo \"POSIXLY=[${POSIXLY_CORRECT-unset}] ENV=[$ENV]\"",
                "exit",
            ],
        );
        let out = &run.output;
        assert_integrated(&run, "RC_BASH_RAN");
        assert!(out.contains("PROFILE_RAN"), "{flag:?}: {out:?}");
        assert!(!out.contains("WRONG_"), "{flag:?}: {out:?}");
        assert!(out.contains("LOGIN_SHELL_YES"), "{flag:?}: {out:?}");
        assert!(out.contains("POSIX_OFF"), "{flag:?}: {out:?}");
        assert!(
            out.contains("POSIXLY=[unset] ENV=[/nonexistent/user-env]"),
            "{flag:?}: {out:?}"
        );
    }
}

#[test]
fn login_bash_falls_back_through_the_login_files() {
    let Some(bash) = find("bash") else {
        eprintln!("skipped: bash is not installed");
        return;
    };
    let s = sandbox();
    // No .bash_profile or .bash_login: .profile runs, and no .bashrc is
    // read because it does not source it.
    std::fs::write(
        s.home.join(".profile"),
        "export USER_VAR=from-rc\necho PROFILE_FALLBACK\n",
    )
    .unwrap();
    std::fs::write(s.home.join(".bashrc"), "echo BASHRC_SHOULD_NOT_RUN\n").unwrap();
    let run = run_shell(
        &bash,
        &s.home,
        &s.runtime,
        &[],
        &["-l"],
        &["echo \"USER_VAR=$USER_VAR\"", "true", "exit"],
    );
    assert_integrated(&run, "PROFILE_FALLBACK");
    assert!(
        !run.output.contains("BASHRC_SHOULD_NOT_RUN"),
        "{:?}",
        run.output
    );
}

#[test]
fn zsh_login_reads_zprofile_and_zlogin_from_the_restored_zdotdir() {
    let Some(zsh) = find("zsh") else {
        eprintln!("skipped: zsh is not installed");
        return;
    };
    let s = sandbox();
    let zdot = s.home.join("zdot");
    std::fs::create_dir(&zdot).unwrap();
    std::fs::write(
        zdot.join(".zprofile"),
        "export USER_VAR=from-rc\necho ZPROFILE_RAN\n",
    )
    .unwrap();
    std::fs::write(zdot.join(".zshrc"), "PS1='%# '\necho RC_ZSH_RAN\n").unwrap();
    std::fs::write(zdot.join(".zlogin"), "echo ZLOGIN_RAN\n").unwrap();
    // Dotfiles in HOME must not be read when ZDOTDIR names another directory.
    std::fs::write(s.home.join(".zprofile"), "echo WRONG_HOME_ZPROFILE\n").unwrap();
    let zdot_str = zdot.to_str().unwrap();
    let run = run_shell(
        &zsh,
        &s.home,
        &s.runtime,
        &[("ZDOTDIR", zdot_str)],
        &["-l"],
        &[
            "echo \"USER_VAR=$USER_VAR\"",
            "[[ -o login ]] && echo LOGIN_YES",
            "echo \"ZDOTDIR_IS=[$ZDOTDIR] SAVED=[${ULTTY_ZSH_ZDOTDIR-unset}]\"",
            "exit",
        ],
    );
    assert_integrated(&run, "ZPROFILE_RAN");
    for needle in ["RC_ZSH_RAN", "ZLOGIN_RAN", "LOGIN_YES"] {
        assert!(run.output.contains(needle), "{needle}: {:?}", run.output);
    }
    assert!(!run.output.contains("WRONG_"), "{:?}", run.output);
    assert!(
        run.output
            .contains(&format!("ZDOTDIR_IS=[{zdot_str}] SAVED=[unset]")),
        "ZDOTDIR not restored:\n{:?}",
        run.output
    );
}

/// A tab opened with a command that is just a shell gets the integration,
/// and a tab running anything else does not.
#[test]
fn a_command_tab_running_a_bare_shell_gets_the_marks() {
    use ultty::terminal_manager::{TabEnv, TerminalManager};
    let Some(bash) = find("bash") else {
        eprintln!("skipped: bash is not installed");
        return;
    };
    let s = sandbox();
    std::fs::write(
        s.home.join(".bashrc"),
        "PS1='$ '\nexport USER_VAR=from-rc\n",
    )
    .unwrap();
    let home = s.home.clone();
    let mut tab_env = move |_| -> std::io::Result<TabEnv> {
        Ok(vec![
            ("HOME".into(), home.clone().into()),
            ("PROMPT_COMMAND".into(), "".into()),
            ("ENV".into(), "".into()),
            ("BASH_ENV".into(), "".into()),
        ])
    };
    let mut marks = |args: &[String]| -> bool {
        let mut manager = TerminalManager::empty(80, 24);
        manager
            .create_tab_with_command(
                &bash,
                args,
                Some(s.home.to_str().unwrap()),
                None,
                &mut tab_env,
            )
            .expect("spawn");
        let id = manager.focused_terminal_id().unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut found = false;
        while Instant::now() < deadline && !found {
            let _ = manager.process_all();
            let terminal = manager.terminal(id).unwrap().event_loop.terminal();
            found = terminal
                .grid()
                .rows()
                .any(|row| row.marks().input_start != 0);
            drop(terminal);
            std::thread::sleep(Duration::from_millis(50));
        }
        found
    };
    assert!(marks(&[]), "bare shell has no prompt marks");
    assert!(
        marks(&["-l".to_string()]),
        "login shell has no prompt marks"
    );
}
