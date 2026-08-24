//! Lightweight markdown rendering for chat bubbles.
//!
//! The TUI ships a full markdown to styled-ANSI pipeline (`src/ui/markdown.rs`)
//! which is overkill here: the GUI has native font weight, italic, underline,
//! background and border support, so we only need to tokenize the source into
//! blocks and re-apply the styles via gpui.
//!
//! Output is a flat `Vec<MarkdownBlock>` with inline spans. Each block is
//! rendered as its own `div`; inline spans within a block are laid out as a
//! horizontal flex row with `flex_wrap` so long paragraphs wrap normally.
//!
//! Supported: ATX headings (h1-h6), paragraphs, fenced / indented code blocks
//! (with language tag preserved), bullet + ordered lists, task lists, block
//! quotes, tables, horizontal rules, inline code, bold, italic, strikethrough,
//! and links (rendered as accent + underline).

use compact_str::CompactString;
use pulldown_cmark::{Alignment, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

#[derive(Debug, Clone)]
pub enum BlockKind {
    Heading(u8),
    Paragraph,
    CodeBlock(Option<CompactString>),
    ListItem(Option<u64>),
    BlockQuote,
    /// (header row, body rows). Header cells are `Vec<MarkdownSpan>` too so
    /// inline styling works inside table cells.
    Table(Vec<Vec<MarkdownSpan>>, Vec<Vec<Vec<MarkdownSpan>>>),
    Hr,
}

#[derive(Debug, Clone, Default)]
pub struct MarkdownSpan {
    pub text: CompactString,
    pub bold: bool,
    pub italic: bool,
    pub strikethrough: bool,
    pub code: bool,
    pub link: Option<CompactString>,
    /// `Some(checked)` when this span is a task-list marker (`[x]` / `[ ]`).
    pub task: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct MarkdownBlock {
    pub kind: BlockKind,
    pub spans: Vec<MarkdownSpan>,
}

/// Parse markdown source into a sequence of renderable blocks.
///
/// The parser state uses a small inline stack to remember which formatting
/// tags (bold / italic / inline code / link) are active when a text event
/// arrives. Tags that don't carry inline state — paragraph, heading,
/// list item, code block, block quote, table cells — mark the start / end
/// of a new block.
pub fn parse_markdown(input: &str) -> Vec<MarkdownBlock> {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_TASKLISTS);
    options.insert(Options::ENABLE_SMART_PUNCTUATION);

    #[derive(Default, Clone)]
    struct Inline {
        bold: bool,
        italic: bool,
        strike: bool,
        code: bool,
        link: Option<CompactString>,
    }

    let parser = Parser::new_ext(input, options);
    let mut blocks: Vec<MarkdownBlock> = Vec::new();
    let mut current_spans: Vec<MarkdownSpan> = Vec::new();
    let mut inline_stack: Vec<Inline> = Vec::new();
    let mut in_code_block: Option<Option<CompactString>> = None;
    let mut list_item_index: Option<u64> = None;
    let mut ordered_next: u64 = 1;
    let mut list_is_ordered: bool = false;

    // Table building state. Cells accumulate into `current_row`; a finished
    // row lands in either the header or the body list.
    let mut in_table: bool = false;
    let mut in_table_head: bool = false;
    let mut table_header: Vec<Vec<MarkdownSpan>> = Vec::new();
    let mut table_body: Vec<Vec<Vec<MarkdownSpan>>> = Vec::new();
    let mut current_row: Vec<Vec<MarkdownSpan>> = Vec::new();
    // Ignored but kept for type-awareness: column alignments from the table
    // tag. Rendering uses them via BlockKind::Table (alignment not carried
    // into spans; kept simple).
    let mut _alignments: Vec<Alignment> = Vec::new();

    let mut push_block = |kind: BlockKind, spans: Vec<MarkdownSpan>| {
        blocks.push(MarkdownBlock { kind, spans });
    };

    for event in parser {
        match event {
            Event::Start(tag) => match tag {
                Tag::Paragraph => {
                    current_spans.clear();
                }
                Tag::Heading { .. } => {
                    current_spans.clear();
                }
                Tag::CodeBlock(kind) => {
                    current_spans.clear();
                    in_code_block = Some(match kind {
                        pulldown_cmark::CodeBlockKind::Indented => None,
                        pulldown_cmark::CodeBlockKind::Fenced(lang) => {
                            let trimmed = lang.split_whitespace().next().unwrap_or("");
                            if trimmed.is_empty() {
                                None
                            } else {
                                Some(CompactString::from(trimmed.to_string()))
                            }
                        }
                    });
                }
                Tag::List(start) => {
                    if let Some(n) = start {
                        ordered_next = n;
                        list_is_ordered = true;
                    } else {
                        ordered_next = 1;
                        list_is_ordered = false;
                    }
                }
                Tag::Item => {
                    current_spans.clear();
                    if list_is_ordered {
                        list_item_index = Some(ordered_next);
                        ordered_next += 1;
                    } else {
                        list_item_index = None;
                    }
                }
                Tag::BlockQuote(_) => {
                    current_spans.clear();
                }
                Tag::Table(aligns) => {
                    _alignments = aligns;
                    in_table = true;
                    in_table_head = false;
                    table_header.clear();
                    table_body.clear();
                    current_row.clear();
                }
                Tag::TableHead => {
                    in_table_head = true;
                    current_row.clear();
                }
                Tag::TableRow => {
                    current_row.clear();
                }
                Tag::TableCell => {
                    current_spans.clear();
                }
                Tag::Emphasis => {
                    let mut s = inline_stack.last().cloned().unwrap_or_default();
                    s.italic = !s.italic;
                    inline_stack.push(s);
                }
                Tag::Strong => {
                    let mut s = inline_stack.last().cloned().unwrap_or_default();
                    s.bold = !s.bold;
                    inline_stack.push(s);
                }
                Tag::Strikethrough => {
                    let mut s = inline_stack.last().cloned().unwrap_or_default();
                    s.strike = !s.strike;
                    inline_stack.push(s);
                }
                Tag::Link { dest_url, .. } => {
                    let mut s = inline_stack.last().cloned().unwrap_or_default();
                    s.link = Some(CompactString::from(dest_url.to_string()));
                    inline_stack.push(s);
                }
                Tag::Image { .. } => {
                    // Render images as their alt text in italics.
                    inline_stack.push(Inline {
                        italic: true,
                        ..inline_stack.last().cloned().unwrap_or_default()
                    });
                }
                _ => {}
            },
            Event::End(tag_end) => match tag_end {
                TagEnd::Paragraph => {
                    push_block(BlockKind::Paragraph, std::mem::take(&mut current_spans));
                }
                TagEnd::Heading(level) => {
                    let n = match level {
                        HeadingLevel::H1 => 1,
                        HeadingLevel::H2 => 2,
                        HeadingLevel::H3 => 3,
                        HeadingLevel::H4 => 4,
                        HeadingLevel::H5 => 5,
                        HeadingLevel::H6 => 6,
                    };
                    push_block(BlockKind::Heading(n), std::mem::take(&mut current_spans));
                }
                TagEnd::CodeBlock => {
                    let lang = in_code_block.take().flatten();
                    push_block(
                        BlockKind::CodeBlock(lang),
                        std::mem::take(&mut current_spans),
                    );
                }
                TagEnd::Item => {
                    let marker = list_item_index.take();
                    push_block(
                        BlockKind::ListItem(marker),
                        std::mem::take(&mut current_spans),
                    );
                }
                TagEnd::BlockQuote(_) => {
                    push_block(BlockKind::BlockQuote, std::mem::take(&mut current_spans));
                }
                TagEnd::TableHead => {
                    in_table_head = false;
                    table_header = std::mem::take(&mut current_row);
                }
                TagEnd::TableRow => {
                    if in_table_head {
                        table_header = std::mem::take(&mut current_row);
                    } else {
                        table_body.push(std::mem::take(&mut current_row));
                    }
                }
                TagEnd::TableCell => {
                    current_row.push(std::mem::take(&mut current_spans));
                }
                TagEnd::Table => {
                    in_table = false;
                    push_block(
                        BlockKind::Table(table_header.clone(), table_body.clone()),
                        Vec::new(),
                    );
                    table_header.clear();
                    table_body.clear();
                }
                TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough | TagEnd::Link => {
                    inline_stack.pop();
                }
                TagEnd::Image => {
                    inline_stack.pop();
                }
                TagEnd::List(_) => {
                    ordered_next = 1;
                }
                _ => {}
            },
            Event::Text(text) => {
                let inl = inline_stack.last().cloned().unwrap_or_default();
                if in_code_block.is_some() {
                    // Code block content: treat as plain, single span. We
                    // accumulate the entire code block into one span (or
                    // append) so the text shows verbatim.
                    let piece = CompactString::from(text.to_string());
                    if let Some(last) = current_spans.last_mut()
                        && last.code
                        && last.link.is_none()
                        && !last.bold
                        && !last.italic
                        && !last.strikethrough
                        && last.task.is_none()
                    {
                        last.text.push_str(&piece);
                        continue;
                    }
                    current_spans.push(MarkdownSpan {
                        text: piece,
                        code: true,
                        ..Default::default()
                    });
                } else {
                    current_spans.push(MarkdownSpan {
                        text: CompactString::from(text.to_string()),
                        bold: inl.bold,
                        italic: inl.italic,
                        strikethrough: inl.strike,
                        code: inl.code,
                        link: inl.link,
                        ..Default::default()
                    });
                }
            }
            Event::Code(text) => {
                current_spans.push(MarkdownSpan {
                    text: CompactString::from(text.to_string()),
                    code: true,
                    ..Default::default()
                });
            }
            Event::SoftBreak | Event::HardBreak => {
                current_spans.push(MarkdownSpan {
                    text: CompactString::from(" "),
                    ..Default::default()
                });
            }
            Event::Rule => {
                push_block(BlockKind::Hr, Vec::new());
            }
            Event::TaskListMarker(checked) => {
                current_spans.push(MarkdownSpan {
                    text: CompactString::from(" "),
                    task: Some(checked),
                    ..Default::default()
                });
            }
            _ => {}
        }
    }

    // Flush any trailing paragraph that wasn't closed by a TagEnd event.
    if !current_spans.is_empty() {
        push_block(BlockKind::Paragraph, std::mem::take(&mut current_spans));
    }

    let _ = in_table; // table always closed by its TagEnd in valid input

    blocks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heading_and_paragraph() {
        let md = "# Title\n\nHello there.";
        let blocks = parse_markdown(md);
        assert!(matches!(blocks[0].kind, BlockKind::Heading(1)));
        assert_eq!(blocks[1].spans.len(), 1);
        assert_eq!(blocks[1].spans[0].text, "Hello there.");
    }

    #[test]
    fn inline_code_bold_italic() {
        let md = "Use `cargo fmt` and **lint** and *italic*.";
        let blocks = parse_markdown(md);
        let block = &blocks[0];
        assert!(matches!(block.kind, BlockKind::Paragraph));
        let has_code = block.spans.iter().any(|s| s.code);
        let has_bold = block.spans.iter().any(|s| s.bold);
        let has_italic = block.spans.iter().any(|s| s.italic);
        assert!(has_code && has_bold && has_italic);
    }

    #[test]
    fn code_block_is_captured_with_lang() {
        let md = "```rust\nfn main() {}\n```";
        let blocks = parse_markdown(md);
        assert!(matches!(&blocks[0].kind, BlockKind::CodeBlock(Some(l)) if l == "rust"));
        let joined: String = blocks[0].spans.iter().map(|s| s.text.as_str()).collect();
        assert!(joined.contains("fn main"));
    }

    #[test]
    fn list_item_marker_carries_number() {
        let md = "1. first\n2. second\n";
        let blocks = parse_markdown(md);
        let markers: Vec<_> = blocks
            .iter()
            .filter_map(|b| match &b.kind {
                BlockKind::ListItem(n) => Some(*n),
                _ => None,
            })
            .collect();
        assert_eq!(markers, vec![Some(1), Some(2)]);
    }

    #[test]
    fn hr_is_emitted() {
        let md = "above\n\n---\n\nbelow\n";
        let blocks = parse_markdown(md);
        assert!(blocks.iter().any(|b| matches!(b.kind, BlockKind::Hr)));
    }

    #[test]
    fn table_is_captured() {
        let md = "| a | b |\n|---|---|\n| 1 | 2 |\n";
        let blocks = parse_markdown(md);
        let table = blocks
            .iter()
            .find_map(|b| match &b.kind {
                BlockKind::Table(header, body) => Some((header, body)),
                _ => None,
            })
            .expect("table block");
        assert_eq!(table.0.len(), 2);
        assert_eq!(table.1.len(), 1);
        assert_eq!(table.1[0].len(), 2);
        let joined: String = table.1[0]
            .iter()
            .flat_map(|c| c.iter())
            .map(|s| s.text.as_str())
            .collect();
        assert_eq!(joined, "12");
    }

    #[test]
    fn task_list_marker_carries_checked() {
        let md = "- [x] done\n- [ ] todo\n";
        let blocks = parse_markdown(md);
        let checked: Vec<_> = blocks
            .iter()
            .flat_map(|b| &b.spans)
            .filter_map(|s| s.task)
            .collect();
        assert_eq!(checked, vec![true, false]);
    }
}
