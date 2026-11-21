//! Keeps `EVENTS.md` honest: each of its JSON examples is a valid event, and
//! together they cover every type in schema order.

use agent_console::event::EventKind;
use agent_console::reader::EventReader;

#[test]
fn events_md_examples_parse_and_cover_every_type() {
    let doc = include_str!("../EVENTS.md");
    let mut examples = String::new();
    let mut in_json = false;
    for line in doc.lines() {
        match line {
            "```json" => in_json = true,
            "```" => in_json = false,
            _ if in_json => {
                examples.push_str(line);
                examples.push('\n');
            }
            _ => {}
        }
    }
    let mut reader = EventReader::new(examples.as_bytes());
    let tags: Vec<_> = reader
        .by_ref()
        .map(|event| event.unwrap().kind.type_name())
        .collect();
    assert!(reader.malformed().is_empty(), "{:?}", reader.malformed());
    assert!(reader.unknown().is_empty(), "{:?}", reader.unknown());
    assert_eq!(tags, EventKind::TYPES);
}
