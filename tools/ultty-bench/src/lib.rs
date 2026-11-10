//! Benchmark harness for Ultty.
//!
//! - [`corpus`]: deterministic, CRLF-only workload generators. The same seed
//!   gives the same bytes on every machine, and each result records the
//!   corpus SHA-256.
//! - [`env`]: the environment metadata every result carries (CPU, governor,
//!   load, pin set, revision), so numbers from different machines or noisy
//!   sessions are never mixed silently.
//! - [`stats`]: the summary statistics the console report prints. A/B
//!   decisions live in `scripts/bench/analyze.py`.
//!
//! The binaries are `ultty-bench` (headless VT throughput, corpus export),
//! and the terminal-neutral drivers `ubench-feed`, `ubench-echo` and
//! `ubench-k2p`, which only speak to a PTY or an X server and so run the
//! same way inside any terminal.

pub mod corpus;
pub mod env;
#[cfg(feature = "proto")]
pub mod proto;
pub mod stats;
pub mod tty;
