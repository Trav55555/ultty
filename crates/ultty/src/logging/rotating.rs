//! A size-capped log file that several processes may append to at once.
//!
//! Every process opens the file with `O_APPEND` and writes whole lines in
//! one `write`, so lines from different processes never interleave. When
//! the file reaches its cap, the process that noticed takes an exclusive
//! `flock` on a lock file next to it and checks that the path still names
//! the file it has open; only then does it shift `ultty.log` to
//! `ultty.log.1` (and `.1` to `.2`, and so on, dropping the oldest). A
//! process that finds the path already names another file knows someone
//! else rotated, and reopens instead of rotating a second time. Before each
//! write a process also checks the path, so it follows another process's
//! rotation instead of writing on into the renamed file.

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use nix::fcntl::{Flock, FlockArg};

/// Mode for the log file and its rotated copies: they may hold paths and
/// window titles.
pub const LOG_FILE_MODE: u32 = 0o600;

/// Mode for a log directory Ultty creates.
pub const LOG_DIR_MODE: u32 = 0o700;

/// A log file rotated by size; see the module documentation.
#[derive(Debug)]
pub struct RotatingFile {
    path: PathBuf,
    max_bytes: u64,
    keep: usize,
    file: File,
}

impl RotatingFile {
    /// Open `path` for appending, creating it (mode 0600) and its missing
    /// parent directories (mode 0700). It is rotated once it holds
    /// `max_bytes`, keeping `keep` older files (`path.1` newest).
    pub fn open(path: &Path, max_bytes: u64, keep: usize) -> io::Result<Self> {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::DirBuilder::new()
                .recursive(true)
                .mode(LOG_DIR_MODE)
                .create(parent)?;
        }
        Ok(Self {
            path: path.to_path_buf(),
            max_bytes,
            keep,
            file: open_append(path)?,
        })
    }

    /// The path this file is written at.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The `index`th rotated copy of the log, `path.<index>`.
    pub fn rotated_path(path: &Path, index: usize) -> PathBuf {
        let mut name = OsString::from(path.as_os_str());
        name.push(format!(".{index}"));
        PathBuf::from(name)
    }

    /// Append `lines`, which must be whole lines, in one write; rotate
    /// afterwards if the file reached its cap.
    pub fn append(&mut self, lines: &[u8]) -> io::Result<()> {
        if !self.path_names_open_file() {
            self.file = open_append(&self.path)?;
        }
        self.file.write_all(lines)?;
        if self.file.metadata()?.len() >= self.max_bytes {
            self.rotate()?;
        }
        Ok(())
    }

    fn path_names_open_file(&self) -> bool {
        match (fs::metadata(&self.path), self.file.metadata()) {
            (Ok(on_disk), Ok(open)) => on_disk.dev() == open.dev() && on_disk.ino() == open.ino(),
            _ => false,
        }
    }

    fn rotate(&mut self) -> io::Result<()> {
        let lock = Flock::lock(
            open_append(&lock_path(&self.path))?,
            FlockArg::LockExclusive,
        )
        .map_err(|(_, errno)| io::Error::from(errno))?;
        if self.path_names_open_file() {
            if self.keep == 0 {
                fs::remove_file(&self.path)?;
            } else {
                for index in (1..self.keep).rev() {
                    let from = Self::rotated_path(&self.path, index);
                    if from.exists() {
                        fs::rename(&from, Self::rotated_path(&self.path, index + 1))?;
                    }
                }
                fs::rename(&self.path, Self::rotated_path(&self.path, 1))?;
            }
        }
        self.file = open_append(&self.path)?;
        drop(lock);
        Ok(())
    }
}

impl Write for RotatingFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.append(buf)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

fn lock_path(path: &Path) -> PathBuf {
    let mut name = OsString::from(path.as_os_str());
    name.push(".lock");
    PathBuf::from(name)
}

fn open_append(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .append(true)
        .create(true)
        .mode(LOG_FILE_MODE)
        .open(path)?;
    file.set_permissions(fs::Permissions::from_mode(LOG_FILE_MODE))?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(path: &Path) -> String {
        fs::read_to_string(path).unwrap_or_default()
    }

    #[test]
    fn test_rotating_file_rotates_at_the_cap_and_keeps_n_old_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ultty.log");
        let mut log = RotatingFile::open(&path, 100, 3).unwrap();
        for batch in 0..6 {
            let line = format!("{batch}{}\n", "x".repeat(98));
            log.append(line.as_bytes()).unwrap();
        }
        assert_eq!(
            read(&path),
            "",
            "the sixth batch filled and rotated the live file"
        );
        for (index, batch) in [(1, 5), (2, 4), (3, 3)] {
            let rotated = read(&RotatingFile::rotated_path(&path, index));
            assert!(
                rotated.starts_with(&batch.to_string()),
                "{index}: {rotated}"
            );
        }
        assert!(!RotatingFile::rotated_path(&path, 4).exists());
    }

    #[test]
    fn test_rotating_file_below_the_cap_does_not_rotate() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ultty.log");
        let mut log = RotatingFile::open(&path, 1024, 3).unwrap();
        log.append(b"one\n").unwrap();
        log.append(b"two\n").unwrap();
        assert_eq!(read(&path), "one\ntwo\n");
        assert!(!RotatingFile::rotated_path(&path, 1).exists());
    }

    #[test]
    fn test_rotating_file_creates_private_file_and_directory() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state").join("ultty").join("ultty.log");
        let mut log = RotatingFile::open(&path, 10, 1).unwrap();
        log.append(b"0123456789\n").unwrap();
        let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&path), LOG_FILE_MODE);
        assert_eq!(mode(&RotatingFile::rotated_path(&path, 1)), LOG_FILE_MODE);
        assert_eq!(mode(path.parent().unwrap()), LOG_DIR_MODE);
        assert_eq!(mode(path.parent().unwrap().parent().unwrap()), LOG_DIR_MODE);
    }

    #[test]
    fn test_rotating_file_tightens_an_existing_files_mode() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ultty.log");
        fs::write(&path, "old\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        RotatingFile::open(&path, 1024, 1).unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            LOG_FILE_MODE
        );
    }

    #[test]
    fn test_rotating_file_follows_another_writers_rotation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ultty.log");
        let mut first = RotatingFile::open(&path, 20, 2).unwrap();
        let mut second = RotatingFile::open(&path, 20, 2).unwrap();
        first.append(b"first fills the cap!\n").unwrap();
        second.append(b"second\n").unwrap();
        assert_eq!(read(&path), "second\n", "second reopened the new file");
        assert_eq!(
            read(&RotatingFile::rotated_path(&path, 1)),
            "first fills the cap!\n"
        );
    }

    /// Each writer stands in for one process: its own open file
    /// description, its own `O_APPEND` offset, its own lock.
    #[test]
    fn test_rotating_file_concurrent_writers_keep_whole_lines_and_lose_none() {
        const WRITERS: usize = 2;
        const LINES: usize = 400;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ultty.log");
        let threads: Vec<_> = (0..WRITERS)
            .map(|writer| {
                let path = path.clone();
                std::thread::spawn(move || {
                    let mut log = RotatingFile::open(&path, 8 * 1024, 8).unwrap();
                    for line in 0..LINES {
                        let text = format!("writer={writer} line={line:04} {}\n", "y".repeat(40));
                        log.append(text.as_bytes()).unwrap();
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }

        let mut seen = std::collections::HashSet::new();
        let files = std::iter::once(path.clone())
            .chain((1..=8).map(|index| RotatingFile::rotated_path(&path, index)));
        let mut rotations = 0;
        for file in files {
            let text = read(&file);
            if file != path && !text.is_empty() {
                rotations += 1;
            }
            assert!(text.is_empty() || text.ends_with('\n'));
            for line in text.lines() {
                let fields: Vec<_> = line.split(' ').collect();
                assert_eq!(fields.len(), 3, "partial or interleaved line: {line:?}");
                assert_eq!(fields[2], "y".repeat(40), "partial line: {line:?}");
                assert!(seen.insert(line.to_owned()), "duplicated line: {line:?}");
            }
        }
        assert_eq!(seen.len(), WRITERS * LINES, "every line survives rotation");
        assert!(
            rotations >= 3,
            "the cap was crossed several times: {rotations}"
        );
    }
}
