//! Convert Markdown to Typst markup.
//!
//! Uses `pulldown_cmark` to parse Markdown and emit a stream of events,
//! translating each into equivalent Typst syntax.

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

/// Convert a Markdown string to a Typst markup string.
pub fn convert(markdown: &str) -> String {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_FOOTNOTES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);

    let parser = Parser::new_ext(markdown, options);
    let mut out = String::new();
    let mut in_code_block = false;
    let mut list_depth: u32 = 0;
    let mut list_is_ordered: Vec<bool> = Vec::new();
    let mut list_counters: Vec<u64> = Vec::new();
    let mut _in_blockquote = false;
    let mut _table_header_done = false;

    for event in parser {
        match event {
            Event::Start(tag) => match tag {
                Tag::Heading { level, .. } => {
                    let hashes = match level {
                        HeadingLevel::H1 => "=",
                        HeadingLevel::H2 => "==",
                        HeadingLevel::H3 => "===",
                        HeadingLevel::H4 => "====",
                        HeadingLevel::H5 => "=====",
                        HeadingLevel::H6 => "======",
                    };
                    out.push_str(hashes);
                    out.push(' ');
                }
                Tag::Paragraph => {}
                Tag::CodeBlock(kind) => {
                    out.push('\n');
                    match kind {
                        CodeBlockKind::Fenced(info) => {
                            out.push_str(&format!("```{info}\n"));
                        }
                        CodeBlockKind::Indented => {
                            out.push_str("```\n");
                        }
                    }
                    in_code_block = true;
                }
                Tag::List(start) => {
                    let is_ordered = start.is_some();
                    list_is_ordered.push(is_ordered);
                    list_counters.push(start.unwrap_or(1));
                    list_depth += 1;
                }
                Tag::Item => {
                    let indent = "  ".repeat((list_depth - 1) as usize);
                    out.push_str(&indent);
                    if let Some(is_ordered) = list_is_ordered.last() {
                        if *is_ordered {
                            let counter = list_counters.last_mut().unwrap();
                            out.push_str(&format!("{counter}. "));
                            *counter += 1;
                        } else {
                            out.push_str("- ");
                        }
                    } else {
                        out.push_str("- ");
                    }
                }
                Tag::BlockQuote(_) => {
                    _in_blockquote = true;
                    out.push_str("#quote[");
                }
                Tag::Table(_) => {
                    _table_header_done = false;
                }
                Tag::TableHead => {
                    _table_header_done = false;
                }
                Tag::TableRow => {}
                Tag::TableCell => {}
                Tag::Strong => out.push_str("#strong["),
                Tag::Emphasis => out.push_str("#emph["),
                Tag::Strikethrough => out.push_str("#strike["),
                Tag::Link { dest_url, .. } => {
                    out.push_str(&format!("#link(\"{}\")[", escape_typst(&dest_url)));
                }
                Tag::Image { dest_url, .. } => {
                    out.push_str(&format!("#image(\"{}\")", escape_typst(&dest_url)));
                }
                Tag::FootnoteDefinition(_) => {}
                Tag::DefinitionList => {}
                Tag::DefinitionListTitle => {}
                Tag::DefinitionListDefinition => {}
                Tag::HtmlBlock => {}
                Tag::MetadataBlock(_) => {}
            },
            Event::End(tag_end) => match tag_end {
                TagEnd::Heading(_) => {
                    out.push('\n');
                }
                TagEnd::Paragraph => {
                    out.push('\n');
                }
                TagEnd::CodeBlock => {
                    out.push_str("```\n");
                    in_code_block = false;
                }
                TagEnd::List(_) => {
                    list_depth -= 1;
                    list_is_ordered.pop();
                    list_counters.pop();
                    if list_depth == 0 {
                        out.push('\n');
                    }
                }
                TagEnd::Item => {
                    out.push('\n');
                }
                TagEnd::BlockQuote(_) => {
                    _in_blockquote = false;
                    out.push_str("]\n");
                }
                TagEnd::TableHead => {
                    _table_header_done = true;
                }
                TagEnd::TableRow => {}
                TagEnd::TableCell => {
                    out.push_str(" | ");
                }
                TagEnd::Table => {
                    out.push('\n');
                }
                TagEnd::Strong => out.push(']'),
                TagEnd::Emphasis => out.push(']'),
                TagEnd::Strikethrough => out.push(']'),
                TagEnd::Link => out.push(']'),
                TagEnd::Image => {}
                TagEnd::FootnoteDefinition => {}
                TagEnd::DefinitionList => {}
                TagEnd::DefinitionListTitle => {}
                TagEnd::DefinitionListDefinition => {}
                TagEnd::HtmlBlock => {}
                TagEnd::MetadataBlock(_) => {}
            },
            Event::Text(text) => {
                if in_code_block {
                    out.push_str(&text);
                } else {
                    out.push_str(&escape_typst(&text));
                }
            }
            Event::Code(code) => {
                out.push_str(&format!("`{}`", escape_typst(&code)));
            }
            Event::Html(html) => {
                out.push_str(&format!("// html: {}", html));
            }
            Event::SoftBreak => {
                out.push('\n');
            }
            Event::HardBreak => {
                out.push_str(" \\");
            }
            Event::Rule => {
                out.push_str("\n---\n\n");
            }
            Event::FootnoteReference(_) => {}
            Event::TaskListMarker(checked) => {
                if checked {
                    out.push_str("[X] ");
                } else {
                    out.push_str("[ ] ");
                }
            }
            Event::InlineMath(math) => {
                out.push_str(&format!("${}$", escape_typst(&math)));
            }
            Event::DisplayMath(math) => {
                out.push_str(&format!("\n${}$\n", escape_typst(&math)));
            }
            Event::InlineHtml(html) => {
                out.push_str(&format!("// html: {}", html));
            }
        }
    }

    out
}

/// Escape special characters for Typst markup.
fn escape_typst(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '#' | '_' | '*' | '[' | ']' | '<' | '>' | '\\' | '`' | '"' | '$' => {
                out.push('\\');
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heading_conversion() {
        let md = "# Title\n## Subtitle\n### Level 3";
        let typst = convert(md);
        assert!(typst.contains("= Title"));
        assert!(typst.contains("== Subtitle"));
        assert!(typst.contains("=== Level 3"));
    }

    #[test]
    fn bold_italic() {
        let md = "This is **bold** and *italic*.";
        let typst = convert(md);
        assert!(typst.contains("#strong[bold]"));
        assert!(typst.contains("#emph[italic]"));
    }

    #[test]
    fn code_block() {
        let md = "```rust\nfn main() {}\n```";
        let typst = convert(md);
        assert!(typst.contains("```rust\nfn main() {}\n```"));
    }

    #[test]
    fn inline_code() {
        let md = "Use `cargo build`.";
        let typst = convert(md);
        assert!(typst.contains("`cargo build`"));
    }

    #[test]
    fn link_conversion() {
        let md = "[Click here](https://example.com)";
        let typst = convert(md);
        assert!(typst.contains("#link(\"https://example.com\")["));
    }

    #[test]
    fn unordered_list() {
        let md = "- Item 1\n- Item 2\n- Item 3";
        let typst = convert(md);
        assert!(typst.contains("- Item 1"));
        assert!(typst.contains("- Item 2"));
    }

    #[test]
    fn ordered_list() {
        let md = "1. First\n2. Second\n3. Third";
        let typst = convert(md);
        assert!(typst.contains("1. First"));
        assert!(typst.contains("2. Second"));
    }

    #[test]
    fn blockquote() {
        let md = "> A wise quote";
        let typst = convert(md);
        assert!(typst.contains("#quote["));
        assert!(typst.contains("A wise quote"));
    }

    #[test]
    fn horizontal_rule() {
        let md = "Before\n\n---\n\nAfter";
        let typst = convert(md);
        assert!(typst.contains("---"));
    }

    #[test]
    fn escaping() {
        let md = "Use #hashtag and _underscore_";
        let typst = convert(md);
        assert!(typst.contains("\\#hashtag"));
    }
}
