//! Replies to Kitty graphics commands.
//!
//! A reply is `ESC _ G i=<id>[,I=<number>][,p=<placement>] ; <message> ESC \`.
//! It carries only the numbers the client sent, printed again as integers,
//! and a code with fixed ASCII text from the table below (ADR 0026,
//! "Replies must not leak"): no payload byte, path, size or internal state
//! ever appears in one, so a `cat`ed file cannot use a reply to type text
//! into the shell. The codes are kitty's own (`kitty/graphics.c`).

/// A failed command: a POSIX-style code and fixed text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Error {
    /// The code, such as `EINVAL`.
    pub code: &'static str,
    /// Fixed, printable ASCII.
    pub text: &'static str,
}

impl Error {
    const fn new(code: &'static str, text: &'static str) -> Self {
        Self { code, text }
    }

    pub const MALFORMED: Self = Self::new("EINVAL", "Malformed control data");
    pub const ID_AND_NUMBER: Self =
        Self::new("EINVAL", "Must not specify both image id and image number");
    pub const UNKNOWN_ACTION: Self = Self::new("EINVAL", "Unknown action");
    pub const ANIMATION: Self = Self::new("EINVAL", "Animation is not supported");
    pub const FEATURE_QUERY: Self = Self::new("EINVAL", "Feature queries are not supported");
    /// File, temporary file and shared memory media: Ultty never reads a
    /// local file or memory object named in terminal output. One code and
    /// text for all three, as the spec requires for every read failure.
    pub const MEDIUM: Self = Self::new("EBADF", "Only direct transmission is supported");
    pub const UNKNOWN_MEDIUM: Self = Self::new("EINVAL", "Unknown transmission type");
    pub const INDEXED: Self = Self::new("EINVAL", "Indexed color images are not supported");
    pub const UNKNOWN_FORMAT: Self = Self::new("EINVAL", "Unknown image format");
    pub const UNKNOWN_COMPRESSION: Self = Self::new("EINVAL", "Unknown image compression");
    pub const PLACEHOLDER: Self = Self::new("EINVAL", "Unicode placeholders are not supported");
    pub const RELATIVE: Self = Self::new("EINVAL", "Relative placements are not supported");
    pub const NO_ID_FOR_QUERY: Self = Self::new("EINVAL", "Query without image id");
    pub const BAD_BASE64: Self = Self::new("EINVAL", "Invalid base64 payload");
    pub const TOO_MUCH_DATA: Self = Self::new("EFBIG", "Too much data");
    pub const NOT_ENOUGH_DATA: Self = Self::new("ENODATA", "Insufficient image data");
    pub const SIZE_MISMATCH: Self = Self::new("EINVAL", "Image dimensions do not match data size");
    pub const ZERO_SIZE: Self = Self::new("EINVAL", "Zero width/height not allowed");
    pub const TOO_LARGE: Self = Self::new("EINVAL", "Image too large");
    pub const INFLATE: Self = Self::new("EINVAL", "Failed to inflate image data");
    pub const PNG: Self = Self::new("EBADPNG", "Failed to decode PNG data");
    pub const NO_SPACE: Self = Self::new("ENOSPC", "Image does not fit in the storage quota");
    pub const NOT_FOUND: Self = Self::new("ENOENT", "Image not found");
    pub const EMPTY_SOURCE: Self = Self::new("EINVAL", "Source rectangle is outside the image");
    pub const ORPHAN_CHUNK: Self = Self::new(
        "EILSEQ",
        "More payload loading refers to non-existent image",
    );
}

/// Who a reply is addressed to: the ids of the command it answers.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Addressee {
    /// `i`
    pub image_id: u32,
    /// `I`
    pub image_number: u32,
    /// `p`
    pub placement_id: u32,
    /// `q`
    pub quiet: u8,
}

/// The reply to send for `outcome`, if any: there is none without an `i`
/// or `I` to address it to, for `OK` under `q=1`, or for anything under
/// `q=2` (spec, "Suppressing responses from the terminal").
pub fn encode(to: Addressee, outcome: Result<(), Error>) -> Option<Vec<u8>> {
    if to.image_id == 0 && to.image_number == 0 {
        return None;
    }
    match (&outcome, to.quiet) {
        (_, 2..) | (Ok(()), 1) => return None,
        _ => {}
    }
    let mut reply = b"\x1b_G".to_vec();
    let mut fields = Vec::with_capacity(3);
    if to.image_id != 0 {
        fields.push(format!("i={}", to.image_id));
    }
    if to.image_number != 0 {
        fields.push(format!("I={}", to.image_number));
    }
    if to.placement_id != 0 {
        fields.push(format!("p={}", to.placement_id));
    }
    reply.extend_from_slice(fields.join(",").as_bytes());
    reply.push(b';');
    match outcome {
        Ok(()) => reply.extend_from_slice(b"OK"),
        Err(error) => {
            reply.extend_from_slice(error.code.as_bytes());
            reply.push(b':');
            reply.extend_from_slice(error.text.as_bytes());
        }
    }
    reply.extend_from_slice(b"\x1b\\");
    Some(reply)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn to(image_id: u32, image_number: u32, placement_id: u32, quiet: u8) -> Addressee {
        Addressee {
            image_id,
            image_number,
            placement_id,
            quiet,
        }
    }

    #[test]
    fn ok_and_errors_carry_the_client_ids() {
        assert_eq!(
            encode(to(31, 0, 0, 0), Ok(())).unwrap(),
            b"\x1b_Gi=31;OK\x1b\\"
        );
        assert_eq!(
            encode(to(99, 13, 7, 0), Ok(())).unwrap(),
            b"\x1b_Gi=99,I=13,p=7;OK\x1b\\"
        );
        assert_eq!(
            encode(to(0, 13, 0, 0), Err(Error::NOT_FOUND)).unwrap(),
            b"\x1b_GI=13;ENOENT:Image not found\x1b\\"
        );
    }

    #[test]
    fn nothing_is_sent_without_an_id() {
        assert_eq!(encode(to(0, 0, 5, 0), Err(Error::MEDIUM)), None);
    }

    #[test]
    fn quiet_modes_suppress_ok_then_errors() {
        assert_eq!(encode(to(1, 0, 0, 1), Ok(())), None);
        assert!(encode(to(1, 0, 0, 1), Err(Error::MEDIUM)).is_some());
        assert_eq!(encode(to(1, 0, 0, 2), Err(Error::MEDIUM)), None);
    }

    #[test]
    fn every_error_text_is_printable_ascii() {
        let all = [
            Error::MALFORMED,
            Error::ID_AND_NUMBER,
            Error::UNKNOWN_ACTION,
            Error::ANIMATION,
            Error::FEATURE_QUERY,
            Error::MEDIUM,
            Error::UNKNOWN_MEDIUM,
            Error::INDEXED,
            Error::UNKNOWN_FORMAT,
            Error::UNKNOWN_COMPRESSION,
            Error::PLACEHOLDER,
            Error::RELATIVE,
            Error::NO_ID_FOR_QUERY,
            Error::BAD_BASE64,
            Error::TOO_MUCH_DATA,
            Error::NOT_ENOUGH_DATA,
            Error::SIZE_MISMATCH,
            Error::ZERO_SIZE,
            Error::TOO_LARGE,
            Error::INFLATE,
            Error::PNG,
            Error::NO_SPACE,
            Error::NOT_FOUND,
            Error::EMPTY_SOURCE,
            Error::ORPHAN_CHUNK,
        ];
        for error in all {
            assert!(error.code.bytes().all(|b| b.is_ascii_uppercase()));
            assert!(error
                .text
                .bytes()
                .all(|b| (0x20..0x7f).contains(&b) && b != b';'));
        }
    }
}
