//! Rewrites `examples/agent-console/fixtures/` from the generator.
//!
//! `cargo run -p agent-console --bin gen-fixtures`

use std::path::Path;

use agent_console::generate::{to_jsonl, FIXTURES};

fn main() -> anyhow::Result<()> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures");
    std::fs::create_dir_all(&dir)?;
    for fixture in FIXTURES {
        let events = (fixture.events)();
        let jsonl = to_jsonl(&events);
        let path = dir.join(fixture.name);
        std::fs::write(&path, &jsonl)?;
        println!(
            "{}: {} events, {} bytes",
            path.display(),
            events.len(),
            jsonl.len()
        );
    }
    Ok(())
}
