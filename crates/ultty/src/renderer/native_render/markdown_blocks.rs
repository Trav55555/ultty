//! Native markdown block planning.

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum MarkdownBlock<'a> {
    CodeBlock {
        language: &'a str,
        lines: Vec<&'a str>,
    },
    Rule,
    Table(Vec<&'a str>),
    Header {
        level: u8,
        text: &'a str,
    },
    Task {
        checked: bool,
        text: &'a str,
    },
    BlockQuote {
        depth: u32,
        text: &'a str,
    },
    OrderedList {
        prefix: String,
        text: &'a str,
    },
    UnorderedList {
        text: &'a str,
    },
    Plain(&'a str),
}

impl MarkdownBlock<'_> {
    pub(super) fn line_count(&self) -> usize {
        match self {
            Self::CodeBlock { lines, .. } | Self::Table(lines) => lines.len(),
            Self::Rule
            | Self::Header { .. }
            | Self::Task { .. }
            | Self::BlockQuote { .. }
            | Self::OrderedList { .. }
            | Self::UnorderedList { .. }
            | Self::Plain(_) => 1,
        }
    }
}

pub(super) fn plan_markdown_blocks(source: &str) -> Vec<MarkdownBlock<'_>> {
    let lines: Vec<&str> = source.lines().collect();
    let mut blocks = Vec::new();
    let mut line_idx = 0;

    while line_idx < lines.len() {
        let line = lines[line_idx];

        if line.starts_with("```") {
            let language = line.trim_start_matches('`').trim();
            let code_start = line_idx + 1;
            line_idx += 1;

            while line_idx < lines.len() && !lines[line_idx].starts_with("```") {
                line_idx += 1;
            }

            blocks.push(MarkdownBlock::CodeBlock {
                language,
                lines: lines[code_start..line_idx].to_vec(),
            });

            if line_idx < lines.len() {
                line_idx += 1;
            }
            continue;
        }

        if line == "---" || line == "***" || line == "___" {
            blocks.push(MarkdownBlock::Rule);
            line_idx += 1;
            continue;
        }

        if line.starts_with('|') && line.matches('|').count() >= 2 {
            let table_start = line_idx;
            while line_idx < lines.len() && lines[line_idx].starts_with('|') {
                line_idx += 1;
            }
            blocks.push(MarkdownBlock::Table(lines[table_start..line_idx].to_vec()));
            continue;
        }

        if let Some(text) = line.strip_prefix("### ") {
            blocks.push(MarkdownBlock::Header { level: 3, text });
            line_idx += 1;
            continue;
        }
        if let Some(text) = line.strip_prefix("## ") {
            blocks.push(MarkdownBlock::Header { level: 2, text });
            line_idx += 1;
            continue;
        }
        if let Some(text) = line.strip_prefix("# ") {
            blocks.push(MarkdownBlock::Header { level: 1, text });
            line_idx += 1;
            continue;
        }

        if let Some(text) = line.strip_prefix("- [ ] ") {
            blocks.push(MarkdownBlock::Task {
                checked: false,
                text,
            });
            line_idx += 1;
            continue;
        }
        if let Some(text) = line
            .strip_prefix("- [x] ")
            .or_else(|| line.strip_prefix("- [X] "))
        {
            blocks.push(MarkdownBlock::Task {
                checked: true,
                text,
            });
            line_idx += 1;
            continue;
        }

        if let Some((depth, text)) = parse_blockquote(line) {
            blocks.push(MarkdownBlock::BlockQuote { depth, text });
            line_idx += 1;
            continue;
        }

        if let Some(text) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) {
            blocks.push(MarkdownBlock::UnorderedList { text });
            line_idx += 1;
            continue;
        }

        if let Some((prefix, text)) = parse_ordered_list(line) {
            blocks.push(MarkdownBlock::OrderedList { prefix, text });
            line_idx += 1;
            continue;
        }

        blocks.push(MarkdownBlock::Plain(line));
        line_idx += 1;
    }

    blocks
}

fn parse_blockquote(line: &str) -> Option<(u32, &str)> {
    let mut depth = 0u32;
    let mut rest = line;
    while let Some(inner) = rest.strip_prefix("> ") {
        depth += 1;
        rest = inner;
    }
    if rest == ">" {
        depth += 1;
        rest = "";
    }

    (depth > 0).then_some((depth, rest))
}

fn parse_ordered_list(line: &str) -> Option<(String, &str)> {
    let dot_pos = line.find(". ")?;
    if dot_pos <= 3 && line[..dot_pos].chars().all(|c| c.is_ascii_digit()) {
        Some((format!("{}. ", &line[..dot_pos]), &line[dot_pos + 2..]))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::{plan_markdown_blocks, MarkdownBlock};

    #[test]
    fn plans_representative_markdown_blocks() {
        let blocks = plan_markdown_blocks(
            "# Title\n---\n| A | B |\n|---|---|\n- [x] done\n1. first\n- bullet\nplain",
        );

        assert!(matches!(
            blocks[0],
            MarkdownBlock::Header {
                level: 1,
                text: "Title"
            }
        ));
        assert_eq!(blocks[1], MarkdownBlock::Rule);
        assert_eq!(
            blocks[2],
            MarkdownBlock::Table(vec!["| A | B |", "|---|---|"])
        );
        assert_eq!(
            blocks[3],
            MarkdownBlock::Task {
                checked: true,
                text: "done"
            }
        );
        assert_eq!(
            blocks[4],
            MarkdownBlock::OrderedList {
                prefix: "1. ".to_string(),
                text: "first"
            }
        );
        assert_eq!(blocks[5], MarkdownBlock::UnorderedList { text: "bullet" });
        assert_eq!(blocks[6], MarkdownBlock::Plain("plain"));
    }

    #[test]
    fn plans_fenced_code_block() {
        let blocks = plan_markdown_blocks("```rust\nfn main() {}\nprintln!();\n```");

        assert_eq!(
            blocks,
            vec![MarkdownBlock::CodeBlock {
                language: "rust",
                lines: vec!["fn main() {}", "println!();"]
            }]
        );
    }

    #[test]
    fn plans_unclosed_fenced_code_block() {
        let blocks = plan_markdown_blocks("```python\nprint('hi')");

        assert_eq!(
            blocks,
            vec![MarkdownBlock::CodeBlock {
                language: "python",
                lines: vec!["print('hi')"]
            }]
        );
    }

    #[test]
    fn plans_nested_blockquote() {
        let blocks = plan_markdown_blocks("> > quoted\n>");

        assert_eq!(
            blocks,
            vec![
                MarkdownBlock::BlockQuote {
                    depth: 2,
                    text: "quoted"
                },
                MarkdownBlock::BlockQuote { depth: 1, text: "" }
            ]
        );
    }
}
