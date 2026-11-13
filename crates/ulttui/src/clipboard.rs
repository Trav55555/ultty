//! Explicit clipboard writes through the rendering backend's existing writer.
//!
//! A successful write only confirms emission. A terminal may reject or ignore
//! clipboard requests; the framework never reads clipboard contents to confirm.

use std::io::Write;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;

/// Maximum UTF-8 payload for a clipboard request, in bytes.
///
/// The 64,000-byte Base64 payload fits Ultty's 65,536-byte OSC buffer, including
/// the OSC 52 header. Oversized text is rejected intact, never truncated.
pub const MAX_CLIPBOARD_BYTES: usize = 48_000;

pub(crate) fn validate(text: &str) -> crate::Result<()> {
    if text.len() > MAX_CLIPBOARD_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("Clipboard text exceeds {MAX_CLIPBOARD_BYTES} UTF-8 bytes"),
        )
        .into());
    }
    Ok(())
}

pub(crate) fn write_osc52(writer: &mut impl Write, text: &str) -> crate::Result<()> {
    validate(text)?;
    // Base64 keeps ESC, BEL, newlines and all other source bytes out of the
    // control-string syntax. This is a write request, never an OSC 52 query.
    let wire = format!("\x1b]52;c;{}\x1b\\", STANDARD.encode(text.as_bytes()));
    writer.write_all(wire.as_bytes())?;
    writer.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_osc52_preserves_unicode_whitespace_and_escape_source() {
        for text in [
            "",
            "\t  code  \n\nlast\n",
            "界e\u{301}👩‍💻\r\n",
            "\x1b]52;c;attacker\x07\x1b\\",
        ] {
            let mut writer = Vec::new();
            write_osc52(&mut writer, text).unwrap();
            assert!(writer.starts_with(b"\x1b]52;c;"));
            assert!(writer.ends_with(b"\x1b\\"));
            let encoded = &writer[7..writer.len() - 2];
            assert!(!encoded.contains(&0x1b));
            assert!(!encoded.contains(&0x07));
            assert_eq!(STANDARD.decode(encoded).unwrap(), text.as_bytes());
        }
    }

    #[test]
    fn test_oversized_clipboard_rejected_before_any_bytes_are_written() {
        let mut writer = Vec::new();
        write_osc52(&mut writer, &"x".repeat(MAX_CLIPBOARD_BYTES)).unwrap();
        assert!(writer.len() < 65_536);
        writer.clear();
        assert!(write_osc52(&mut writer, &"x".repeat(MAX_CLIPBOARD_BYTES + 1)).is_err());
        assert!(writer.is_empty());
        assert!(validate(&"界".repeat(MAX_CLIPBOARD_BYTES / 3 + 1)).is_err());
    }

    #[test]
    fn test_osc52_propagates_writer_and_flush_failures() {
        struct FailedWriter {
            flush: bool,
        }
        impl Write for FailedWriter {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                if self.flush {
                    Ok(bytes.len())
                } else {
                    Err(std::io::ErrorKind::BrokenPipe.into())
                }
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Err(std::io::ErrorKind::BrokenPipe.into())
            }
        }
        assert!(write_osc52(&mut FailedWriter { flush: false }, "source").is_err());
        assert!(write_osc52(&mut FailedWriter { flush: true }, "source").is_err());
    }
}
