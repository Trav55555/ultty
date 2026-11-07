//! Deterministic workload corpora.
//!
//! Every corpus is CRLF: a bare `\n` makes a terminal move down without
//! returning to column 0, so the output staircases across the screen and the
//! measurement no longer resembles `cat` through a PTY with ONLCR (it changed
//! headless throughput by about 45% in the 2026-11-07 probe). [`generate`]
//! asserts this for every corpus it returns.
//!
//! Generation stops at the first line (or frame) boundary at or past the
//! requested size, so escape sequences are never cut in half. The sizes are
//! therefore approximate; results record the exact length and SHA-256.

use sha2::{Digest, Sha256};

/// Seed shared by every corpus. Changing it changes every hash.
pub const SEED: u64 = 0x5eed_2026_1107;

/// Nominal screen the cursor-addressed corpora are written for. Smaller
/// screens clamp the addresses; that is still valid input.
pub const AUTHORED_COLS: u16 = 120;
pub const AUTHORED_ROWS: u16 = 40;

/// One workload family.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Corpus {
    /// 84-column prose lines.
    Ascii,
    /// One SGR change every 1-4 cells: 256-color, truecolor, attributes.
    SgrDense,
    /// CJK, emoji, combining marks, Greek, Cyrillic, Hebrew and Arabic mixed.
    UnicodeMixed,
    /// Double-width CJK ideographs and kana only.
    Cjk,
    /// Emoji with skin tones, ZWJ sequences, flags and VS16.
    EmojiZwj,
    /// Latin bases with one to three combining marks each.
    Combining,
    /// htop/vim-style partial redraws: CUP, SGR, EL, no newlines.
    CursorRedraw,
    /// Short `seq`-like lines: line-at-a-time scrolling into history.
    Scroll,
    /// DECSTBM region with IND/RI storms, as a pager scrolls.
    ScrollRegion,
    /// Full-screen frames on the alternate screen inside `?2026` sync updates.
    AltScreen,
    /// 200-4000 character lines that soft-wrap.
    WideWrap,
}

impl Corpus {
    pub const ALL: [Corpus; 11] = [
        Corpus::Ascii,
        Corpus::SgrDense,
        Corpus::UnicodeMixed,
        Corpus::Cjk,
        Corpus::EmojiZwj,
        Corpus::Combining,
        Corpus::CursorRedraw,
        Corpus::Scroll,
        Corpus::ScrollRegion,
        Corpus::AltScreen,
        Corpus::WideWrap,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Corpus::Ascii => "ascii",
            Corpus::SgrDense => "sgr-dense",
            Corpus::UnicodeMixed => "unicode-mixed",
            Corpus::Cjk => "cjk",
            Corpus::EmojiZwj => "emoji-zwj",
            Corpus::Combining => "combining",
            Corpus::CursorRedraw => "cursor-redraw",
            Corpus::Scroll => "scroll",
            Corpus::ScrollRegion => "scroll-region",
            Corpus::AltScreen => "alt-screen",
            Corpus::WideWrap => "wide-wrap",
        }
    }

    pub fn from_name(name: &str) -> Option<Corpus> {
        Corpus::ALL.into_iter().find(|c| c.name() == name)
    }
}

/// A generated corpus and its identity.
pub struct Generated {
    pub corpus: Corpus,
    pub bytes: Vec<u8>,
    pub sha256: String,
}

/// Generate `corpus` with at least `target_len` bytes (see the module docs).
///
/// # Panics
///
/// Panics if a generator emits a bare `\n`; that is a bug in this module.
pub fn generate(corpus: Corpus, target_len: usize) -> Generated {
    let mut gen = Gen {
        rng: Rng::new(SEED ^ (corpus as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15)),
        out: Vec::with_capacity(target_len + 8 * 1024),
    };
    match corpus {
        Corpus::Ascii => gen.lines(target_len, Gen::ascii_line),
        Corpus::SgrDense => gen.lines(target_len, Gen::sgr_line),
        Corpus::UnicodeMixed => gen.lines(target_len, Gen::unicode_mixed_line),
        Corpus::Cjk => gen.lines(target_len, Gen::cjk_line),
        Corpus::EmojiZwj => gen.lines(target_len, Gen::emoji_line),
        Corpus::Combining => gen.lines(target_len, Gen::combining_line),
        Corpus::CursorRedraw => gen.cursor_redraw(target_len),
        Corpus::Scroll => gen.lines(target_len, Gen::scroll_line),
        Corpus::ScrollRegion => gen.scroll_region(target_len),
        Corpus::AltScreen => gen.alt_screen(target_len),
        Corpus::WideWrap => gen.lines(target_len, Gen::wide_line),
    }
    let bytes = gen.out;
    if let Some(offset) = first_bare_lf(&bytes) {
        panic!("corpus {} has a bare LF at byte {offset}", corpus.name());
    }
    let sha256 = sha256_hex(&bytes);
    Generated {
        corpus,
        bytes,
        sha256,
    }
}

/// Offset of the first `\n` not preceded by `\r`, if any.
pub fn first_bare_lf(bytes: &[u8]) -> Option<usize> {
    bytes
        .iter()
        .enumerate()
        .find(|&(i, &b)| b == b'\n' && (i == 0 || bytes[i - 1] != b'\r'))
        .map(|(i, _)| i)
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// SplitMix64: tiny, fast, and identical on every platform.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Uniform-enough value in `lo..=hi` (modulo bias is irrelevant here).
    fn range(&mut self, lo: u32, hi: u32) -> u32 {
        lo + (self.next_u64() % u64::from(hi - lo + 1)) as u32
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[(self.next_u64() % items.len() as u64) as usize]
    }

    fn chance(&mut self, percent: u32) -> bool {
        self.range(0, 99) < percent
    }
}

const WORDS: &[&str] = &[
    "the",
    "quick",
    "brown",
    "fox",
    "jumps",
    "over",
    "lazy",
    "dog",
    "terminal",
    "render",
    "glyph",
    "buffer",
    "scroll",
    "cursor",
    "error:",
    "warning:",
    "compiling",
    "finished",
    "release",
    "target",
    "src/main.rs:42:7",
    "0x7ffd5a3c",
    "12.34ms",
    "[ok]",
    "{\"id\":17}",
    "--verbose",
    "Result<(), Error>",
    "let",
    "fn",
    "match",
    "=>",
    "&&",
    "||",
    "200",
    "GET",
    "/api/v1/items",
];

const CJK_WORDS: &[&str] = &[
    "日本語",
    "中文字符",
    "한국어",
    "東京都",
    "ひらがな",
    "カタカナ",
    "漢字",
    "終端",
    "文字化け",
    "表示",
    "速度",
    "测试",
];
const EMOJI: &[&str] = &[
    "😀",
    "🚀",
    "🎨",
    "🔥",
    "👍🏽",
    "👨‍👩‍👧‍👦",
    "🏳️‍🌈",
    "❤️",
    "🇯🇵",
    "🇺🇸",
    "🧑‍💻",
    "✨",
    "🐛",
    "✅",
];
const COMBINING_WORDS: &[&str] = &["é̃", "à̂", "ñ̈", "ö̧", "ụ̃", "Ặ", "ǘ", "x̧̌"];
const OTHER_SCRIPTS: &[&str] = &[
    "αβγδε",
    "Ελληνικά",
    "Кириллица",
    "привет",
    "שלום",
    "עברית",
    "مرحبا",
    "العربية",
    "हिन्दी",
];

struct Gen {
    rng: Rng,
    out: Vec<u8>,
}

impl Gen {
    fn lines(&mut self, target_len: usize, mut line: impl FnMut(&mut Gen)) {
        while self.out.len() < target_len {
            line(self);
            self.out.extend_from_slice(b"\r\n");
        }
    }

    fn push(&mut self, s: &str) {
        self.out.extend_from_slice(s.as_bytes());
    }

    fn ascii_line(&mut self) {
        let start = self.out.len();
        while self.out.len() - start < 84 {
            let word = *self.rng.pick(WORDS);
            self.push(word);
            self.out.push(b' ');
        }
        self.out.truncate(start + 84);
    }

    fn sgr(&mut self) {
        let sgr = match self.rng.range(0, 6) {
            0 => format!("\x1b[38;5;{}m", self.rng.range(0, 255)),
            1 => format!(
                "\x1b[38;2;{};{};{}m",
                self.rng.range(0, 255),
                self.rng.range(0, 255),
                self.rng.range(0, 255)
            ),
            2 => format!("\x1b[48;5;{}m", self.rng.range(0, 255)),
            3 => format!("\x1b[{};{}m", self.rng.range(1, 4), self.rng.range(30, 37)),
            4 => format!(
                "\x1b[48;2;{};{};{}m",
                self.rng.range(0, 64),
                self.rng.range(0, 64),
                self.rng.range(0, 64)
            ),
            5 => "\x1b[0m".to_string(),
            _ => "\x1b[1;3;4m".to_string(),
        };
        self.push(&sgr);
    }

    fn sgr_line(&mut self) {
        let width = self.rng.range(60, 118);
        let mut cells = 0;
        while cells < width {
            self.sgr();
            let run = self.rng.range(1, 4);
            for _ in 0..run {
                let c = b'a' + self.rng.range(0, 25) as u8;
                self.out.push(c);
            }
            cells += run;
        }
        self.push("\x1b[0m");
    }

    /// Words from `pool`, each followed by a space, until about `cols`
    /// columns at `width` columns per character.
    fn words_line(&mut self, pool: &[&str], chars: usize) {
        let mut n = 0;
        while n < chars {
            let word = *self.rng.pick(pool);
            self.push(word);
            self.out.push(b' ');
            n += word.chars().count() + 1;
        }
    }

    fn cjk_line(&mut self) {
        // About 40 double-width characters: 80 columns.
        self.words_line(CJK_WORDS, 27);
    }

    fn emoji_line(&mut self) {
        let count = self.rng.range(20, 36);
        for _ in 0..count {
            let e = *self.rng.pick(EMOJI);
            self.push(e);
        }
    }

    fn combining_line(&mut self) {
        let count = self.rng.range(40, 80);
        for _ in 0..count {
            self.out.push(b'a' + self.rng.range(0, 25) as u8);
            for _ in 0..self.rng.range(1, 3) {
                let mark = char::from_u32(0x300 + self.rng.range(0, 0x6f)).unwrap_or('\u{301}');
                let mut buf = [0u8; 4];
                self.push(mark.encode_utf8(&mut buf));
            }
            if self.rng.chance(15) {
                self.out.push(b' ');
            }
        }
    }

    fn unicode_mixed_line(&mut self) {
        let mut n = 0;
        while n < 60 {
            let word = match self.rng.range(0, 5) {
                0 => *self.rng.pick(WORDS),
                1 => *self.rng.pick(CJK_WORDS),
                2 => *self.rng.pick(EMOJI),
                3 => *self.rng.pick(COMBINING_WORDS),
                _ => *self.rng.pick(OTHER_SCRIPTS),
            };
            self.push(word);
            self.out.push(b' ');
            n += word.chars().count() + 1;
        }
    }

    fn scroll_line(&mut self) {
        let n = self.rng.range(0, 999_999);
        self.push(&n.to_string());
    }

    fn wide_line(&mut self) {
        let len = self.rng.range(200, 4000) as usize;
        let start = self.out.len();
        while self.out.len() - start < len {
            let word = *self.rng.pick(WORDS);
            self.push(word);
            self.out.push(b' ');
        }
        self.out.truncate(start + len);
    }

    fn cup(&mut self, row: u32, col: u32) {
        self.push(&format!("\x1b[{row};{col}H"));
    }

    /// htop-like: a header of meters and a process table, refreshed by
    /// rewriting a few fields at a time; vim-like: whole-screen repaint of
    /// lines with EL after each.
    fn cursor_redraw(&mut self, target_len: usize) {
        let rows = u32::from(AUTHORED_ROWS);
        self.push("\x1b[H\x1b[2J");
        while self.out.len() < target_len {
            if self.rng.chance(80) {
                // Partial refresh: a handful of fields.
                for _ in 0..self.rng.range(4, 24) {
                    let row = self.rng.range(1, rows);
                    let col = self.rng.range(1, u32::from(AUTHORED_COLS) - 20);
                    self.cup(row, col);
                    self.sgr();
                    let width = self.rng.range(4, 18) as usize;
                    let start = self.out.len();
                    while self.out.len() - start < width {
                        self.out.push(b'0' + self.rng.range(0, 9) as u8);
                        if self.rng.chance(20) {
                            self.out.push(b'.');
                        }
                    }
                    if self.rng.chance(30) {
                        self.push("\x1b[K");
                    }
                }
                self.push("\x1b[0m");
            } else {
                // Full repaint, as vim does on a page scroll.
                for row in 1..=rows {
                    self.cup(row, 1);
                    self.ascii_line();
                    self.push("\x1b[K");
                }
            }
            // Park the cursor like a status line would.
            self.cup(rows, 1);
        }
    }

    fn scroll_region(&mut self, target_len: usize) {
        let (top, bottom) = (2u32, u32::from(AUTHORED_ROWS) - 1);
        while self.out.len() < target_len {
            self.push(&format!("\x1b[{top};{bottom}r"));
            // Forward scrolling: write at the bottom line, then IND.
            for _ in 0..self.rng.range(10, 60) {
                self.cup(bottom, 1);
                self.ascii_line();
                self.push("\x1b[K\x1bD");
            }
            // Backward scrolling: RI at the top line, then write it.
            for _ in 0..self.rng.range(5, 30) {
                self.cup(top, 1);
                self.push("\x1bM");
                self.ascii_line();
                self.push("\x1b[K");
            }
            self.push("\x1b[r");
        }
    }

    fn alt_screen(&mut self, target_len: usize) {
        self.push("\x1b[?1049h");
        while self.out.len() < target_len {
            self.push("\x1b[?2026h\x1b[H");
            for row in 1..=u32::from(AUTHORED_ROWS) {
                self.cup(row, 1);
                if self.rng.chance(50) {
                    self.sgr_line();
                } else {
                    self.ascii_line();
                }
                self.push("\x1b[K");
            }
            self.push("\x1b[?2026l");
        }
        self.push("\x1b[?1049l");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIB: usize = 1024 * 1024;

    #[test]
    fn every_corpus_is_crlf_and_reaches_its_size() {
        for corpus in Corpus::ALL {
            let g = generate(corpus, MIB);
            assert!(g.bytes.len() >= MIB, "{} is short", corpus.name());
            assert!(
                g.bytes.len() < MIB + 64 * 1024,
                "{} overshoots: {}",
                corpus.name(),
                g.bytes.len()
            );
            assert_eq!(first_bare_lf(&g.bytes), None, "{}", corpus.name());
            assert!(std::str::from_utf8(&g.bytes).is_ok(), "{}", corpus.name());
        }
    }

    #[test]
    fn generation_is_deterministic() {
        for corpus in Corpus::ALL {
            assert_eq!(
                generate(corpus, 256 * 1024).sha256,
                generate(corpus, 256 * 1024).sha256,
                "{}",
                corpus.name()
            );
        }
    }

    /// Results from different sessions are only comparable when the bytes
    /// are the same. A generator change must update these on purpose (and
    /// invalidates older results for that corpus).
    #[test]
    fn one_mib_hashes_are_pinned() {
        let pinned = [
            (
                Corpus::Ascii,
                "ededa46d283a260d3fcba4b35f88b1213242913e3001add537163100e09b9b8f",
            ),
            (
                Corpus::SgrDense,
                "7fa3047af54f0a8a5c7abe10df732728f54016d231525dac571336be83b0a752",
            ),
            (
                Corpus::UnicodeMixed,
                "cb27ae80e0d58149d01a8da33ed3ee69f827f25705aefa9eb29947e13d2ac561",
            ),
            (
                Corpus::Cjk,
                "4a07da092a49eef12f8a163068114075ab54da6e4d27c262c6fa60a39a00731c",
            ),
            (
                Corpus::EmojiZwj,
                "7a12ab38b4dc34ee57c647ce0a4d90fd98d04d027dfe4ec8ecd8508a072193b8",
            ),
            (
                Corpus::Combining,
                "246b8911d7b4a3ceaa7e4c4e3938d7c64f42cdc74b635838ae0cd35495b6178b",
            ),
            (
                Corpus::CursorRedraw,
                "370274e4ac73e793a92a604d47ddbdf98f1690847608981921817492f47aa733",
            ),
            (
                Corpus::Scroll,
                "db07a3d9f1c6ee20d37f4b64bc4f31b8360352392ee97f9c28602c2f9cf2fc00",
            ),
            (
                Corpus::ScrollRegion,
                "71f06a565526cc726fe662a04f2ec19864c2dd9578a95574ab36adad30b1db5e",
            ),
            (
                Corpus::AltScreen,
                "5abf2211133655cc78a90c84f6c2c617f8d9177f3ed829aa460ed849ea732151",
            ),
            (
                Corpus::WideWrap,
                "c8810f1d114a314273fd765e69af61511a75154fe63f995daf093b0c291b2ed1",
            ),
        ];
        assert_eq!(pinned.len(), Corpus::ALL.len());
        for (corpus, sha) in pinned {
            assert_eq!(generate(corpus, MIB).sha256, sha, "{}", corpus.name());
        }
    }

    #[test]
    fn corpora_differ_from_each_other() {
        let mut hashes: Vec<_> = Corpus::ALL
            .into_iter()
            .map(|c| generate(c, 64 * 1024).sha256)
            .collect();
        hashes.sort();
        hashes.dedup();
        assert_eq!(hashes.len(), Corpus::ALL.len());
    }

    #[test]
    fn bare_lf_detection() {
        assert_eq!(first_bare_lf(b"a\r\nb\r\n"), None);
        assert_eq!(first_bare_lf(b"a\r\nb\nc"), Some(4));
        assert_eq!(first_bare_lf(b"\nx"), Some(0));
    }

    #[test]
    fn names_round_trip() {
        for corpus in Corpus::ALL {
            assert_eq!(Corpus::from_name(corpus.name()), Some(corpus));
        }
        assert_eq!(Corpus::from_name("nope"), None);
    }
}
