//! Tabulates the per-codepoint properties of `src/grid/char_class.rs` for
//! the codepoints below [`LIMIT`], so printing a character finds its width
//! and grapheme cluster class in two loads instead of two searches.
//!
//! The table has two levels: an index from each block of [`BLOCK`]
//! codepoints to one copy of every distinct block. `cluster.rs` checks it
//! against `props_bits` for every codepoint.

use std::fmt::Write as _;

#[path = "src/grid/char_class.rs"]
mod char_class;

/// Codepoints below this are in the table: the BMP, the emoji in plane 1
/// and the CJK ideographs in planes 2 and 3.
const LIMIT: u32 = 0x40000;
/// Codepoints per block.
const BLOCK: u32 = 64;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/grid/char_class.rs");

    let mut blocks: Vec<Vec<u8>> = Vec::new();
    let mut index = Vec::new();
    for start in (0..LIMIT).step_by(BLOCK as usize) {
        let block: Vec<u8> = (start..start + BLOCK)
            .map(|cp| char::from_u32(cp).map_or(0, char_class::props_bits))
            .collect();
        let at = blocks.iter().position(|b| *b == block).unwrap_or_else(|| {
            blocks.push(block);
            blocks.len() - 1
        });
        index.push(u8::try_from(at).expect("more than 256 distinct blocks"));
    }

    let mut out = String::new();
    writeln!(out, "/// Codepoints below this are in the table.").unwrap();
    writeln!(out, "const TABLE_LIMIT: u32 = {LIMIT:#X};").unwrap();
    writeln!(out, "/// log2 of the codepoints per block.").unwrap();
    writeln!(out, "const BLOCK_SHIFT: u32 = {};", BLOCK.trailing_zeros()).unwrap();
    writeln!(out, "/// The block holding each run of codepoints.").unwrap();
    writeln!(
        out,
        "static BLOCK_INDEX: [u8; {}] = {index:?};",
        index.len()
    )
    .unwrap();
    writeln!(out, "/// Every distinct block of properties bytes.").unwrap();
    writeln!(
        out,
        "static BLOCKS: [[u8; {BLOCK}]; {}] = {blocks:?};",
        blocks.len()
    )
    .unwrap();

    let path = std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("char_props.rs");
    std::fs::write(path, out).unwrap();
}
