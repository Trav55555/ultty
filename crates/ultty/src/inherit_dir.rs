//! Where a new tab or split opens: the directory of the pane it was made from.
//!
//! The shell tells the terminal its directory with OSC 7
//! (`file://host/path`). That is only a hint: the host may be another machine
//! (an ssh session), the path may no longer exist, and the shell may not send
//! OSC 7 at all. This module holds the decision, separate from the process and
//! filesystem work, so it can be tested without a pty.

use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;

/// The local directory an OSC 7 URI names, if it names one on `hostname`.
///
/// Accepts `file://` URIs whose host is empty, `localhost` or `hostname`
/// (case-insensitive), with an absolute path. The path is percent-decoded as
/// bytes, so a non-UTF-8 name survives. Anything else is `None`: other
/// schemes, remote hosts, relative paths, and paths with a NUL byte.
pub fn local_dir_from_osc7(uri: &str, hostname: &str) -> Option<PathBuf> {
    let rest = uri.get(..7).filter(|s| s.eq_ignore_ascii_case("file://"))?;
    let rest = &uri[rest.len()..];
    let path_start = rest.find('/')?;
    let (host, path) = rest.split_at(path_start);
    let local = host.is_empty()
        || host.eq_ignore_ascii_case("localhost")
        || (!hostname.is_empty() && host.eq_ignore_ascii_case(hostname));
    if !local {
        return None;
    }
    let bytes = percent_decode_bytes(path);
    if bytes.contains(&0) {
        return None;
    }
    Some(PathBuf::from(OsString::from_vec(bytes)))
}

fn percent_decode_bytes(input: &str) -> Vec<u8> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

/// The directory to start a new pane in, or `None` to use Ultty's own.
///
/// Prefers the pane's last OSC 7 directory when it is local and still a
/// directory; otherwise `process_dir` (the pane's foreground process, read
/// lazily because it costs a `/proc` lookup); otherwise `None`.
pub fn pick(
    osc7_uri: Option<&str>,
    hostname: &str,
    process_dir: impl FnOnce() -> Option<PathBuf>,
) -> Option<PathBuf> {
    osc7_uri
        .and_then(|uri| local_dir_from_osc7(uri, hostname))
        .filter(|dir| dir.is_dir())
        .or_else(|| process_dir().filter(|dir| dir.is_dir()))
}

/// This machine's hostname, or empty when it cannot be read.
pub fn hostname() -> String {
    nix::unistd::gethostname()
        .ok()
        .and_then(|name| name.into_string().ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOST: &str = "box";

    fn dir(uri: &str) -> Option<String> {
        local_dir_from_osc7(uri, HOST).map(|p| p.to_string_lossy().into_owned())
    }

    #[test]
    fn empty_localhost_and_own_host_are_local() {
        assert_eq!(dir("file:///home/a").as_deref(), Some("/home/a"));
        assert_eq!(dir("file://localhost/home/a").as_deref(), Some("/home/a"));
        assert_eq!(dir("file://LocalHost/home/a").as_deref(), Some("/home/a"));
        assert_eq!(dir("file://box/home/a").as_deref(), Some("/home/a"));
        assert_eq!(dir("FILE://Box/home/a").as_deref(), Some("/home/a"));
    }

    #[test]
    fn other_hosts_and_schemes_are_not_local() {
        assert_eq!(dir("file://other/home/a"), None);
        assert_eq!(dir("file://box.example.com/home/a"), None);
        assert_eq!(dir("file://user@box/home/a"), None);
        assert_eq!(dir("http://box/home/a"), None);
        assert_eq!(dir("/home/a"), None);
    }

    #[test]
    fn unknown_hostname_only_accepts_empty_and_localhost() {
        assert_eq!(local_dir_from_osc7("file:///x", ""), Some("/x".into()));
        assert_eq!(local_dir_from_osc7("file:///x", "box"), Some("/x".into()));
        assert_eq!(local_dir_from_osc7("file://box/x", ""), None);
    }

    #[test]
    fn path_is_percent_decoded() {
        assert_eq!(dir("file:///a%20b/caf%C3%A9").as_deref(), Some("/a b/café"));
        assert_eq!(dir("file:///100%25").as_deref(), Some("/100%"));
        // Bad escapes stay as written, like the terminal's own decoder.
        assert_eq!(dir("file:///a%zz/b%2").as_deref(), Some("/a%zz/b%2"));
    }

    #[test]
    fn non_utf8_bytes_survive_decoding() {
        let path = local_dir_from_osc7("file:///a%FFb", HOST).unwrap();
        assert_eq!(path.into_os_string().into_vec(), b"/a\xffb");
    }

    #[test]
    fn missing_path_and_nul_are_rejected() {
        assert_eq!(dir("file://box"), None);
        assert_eq!(dir("file://"), None);
        assert_eq!(dir("file:///a%00b"), None);
    }

    #[test]
    fn pick_prefers_a_live_osc7_directory() {
        let osc = tempfile::tempdir().unwrap();
        let proc = tempfile::tempdir().unwrap();
        let uri = format!("file://{}", osc.path().display());
        let picked = pick(Some(&uri), HOST, || Some(proc.path().to_path_buf()));
        assert_eq!(picked.as_deref(), Some(osc.path()));
    }

    #[test]
    fn pick_falls_back_to_the_process_when_osc7_is_unusable() {
        let proc = tempfile::tempdir().unwrap();
        let fallback = || Some(proc.path().to_path_buf());
        // No OSC 7.
        assert_eq!(pick(None, HOST, fallback).as_deref(), Some(proc.path()));
        // A remote host.
        assert_eq!(
            pick(Some("file://other/tmp"), HOST, fallback).as_deref(),
            Some(proc.path())
        );
        // A directory that is gone.
        let gone = tempfile::tempdir().unwrap();
        let uri = format!("file://{}", gone.path().display());
        drop(gone);
        assert_eq!(
            pick(Some(&uri), HOST, fallback).as_deref(),
            Some(proc.path())
        );
    }

    #[test]
    fn pick_is_none_when_nothing_is_usable() {
        assert_eq!(pick(None, HOST, || None), None);
        assert_eq!(pick(None, HOST, || Some("/nonexistent/zz".into())), None);
    }

    #[test]
    fn pick_skips_the_process_lookup_when_osc7_works() {
        let osc = tempfile::tempdir().unwrap();
        let uri = format!("file://{}", osc.path().display());
        let picked = pick(Some(&uri), HOST, || panic!("process cwd read needlessly"));
        assert!(picked.is_some());
    }
}
