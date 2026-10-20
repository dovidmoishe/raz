//! Render a conservative subset of CommonMark to independently valid Telegram HTML.
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// HTML and matching plain text, for a formatting-rejection fallback.
/// Bound serialized HTML bytes as well as displayed UTF-16 units. This leaves
/// room for part numbers and keeps entities, Unicode scalars and tags intact.
pub fn reply_parts(markdown: &str) -> Vec<(String, String)> {
    let mut parts = Vec::new();
    let mut html = String::new();
    let mut plain = String::new();
    let mut stack: Vec<(String, String)> = Vec::new();
    let mut lists: Vec<Option<u64>> = Vec::new();
    for event in Parser::new_ext(markdown, Options::ENABLE_STRIKETHROUGH) {
        match event {
            Event::Start(tag) => {
                let (open, close) = match tag {
                    Tag::Strong | Tag::Heading { .. } => ("<b>".into(), "</b>".into()),
                    Tag::Emphasis => ("<i>".into(), "</i>".into()),
                    Tag::Strikethrough => ("<s>".into(), "</s>".into()),
                    Tag::CodeBlock(_) => ("<pre>".into(), "</pre>".into()),
                    Tag::Link { dest_url, .. }
                        if dest_url.len() < 512
                            && ["https://", "http://", "tg://", "mailto:"]
                                .iter()
                                .any(|p| dest_url.starts_with(p)) =>
                    {
                        (format!("<a href=\"{}\">", escape(&dest_url)), "</a>".into())
                    }
                    Tag::List(start) => {
                        lists.push(start);
                        (String::new(), String::new())
                    }
                    Tag::Item => {
                        let prefix = match lists.last_mut() {
                            Some(Some(n)) => {
                                let prefix = format!("{n}. ");
                                *n += 1;
                                prefix
                            }
                            _ => "• ".into(),
                        };
                        append(&prefix, &stack, &mut html, &mut plain, &mut parts);
                        (String::new(), String::new())
                    }
                    _ => (String::new(), String::new()),
                };
                stack.push((open, close));
            }
            Event::End(tag) => {
                stack.pop();
                if matches!(tag, TagEnd::List(_)) {
                    lists.pop();
                }
                if matches!(
                    tag,
                    TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::CodeBlock | TagEnd::Item
                ) {
                    append("\n", &stack, &mut html, &mut plain, &mut parts);
                }
                if matches!(tag, TagEnd::Paragraph | TagEnd::Heading(_)) {
                    append("\n", &stack, &mut html, &mut plain, &mut parts);
                }
            }
            Event::Text(text) | Event::Html(text) | Event::InlineHtml(text) => {
                append(&text, &stack, &mut html, &mut plain, &mut parts)
            }
            Event::Code(text) => {
                stack.push(("<code>".into(), "</code>".into()));
                append(&text, &stack, &mut html, &mut plain, &mut parts);
                stack.pop();
            }
            Event::SoftBreak | Event::HardBreak => {
                append("\n", &stack, &mut html, &mut plain, &mut parts)
            }
            Event::Rule => append("———\n", &stack, &mut html, &mut plain, &mut parts),
            _ => {}
        }
    }
    if !plain.trim().is_empty() {
        parts.push((html, plain));
    }
    if parts.is_empty() {
        parts.push(("(empty)".into(), "(empty)".into()));
    }
    parts
}

fn append(
    text: &str,
    stack: &[(String, String)],
    html: &mut String,
    plain: &mut String,
    parts: &mut Vec<(String, String)>,
) {
    // Each text run is wrapped independently, so every split has balanced tags.
    let open: String = stack.iter().map(|(s, _)| s.as_str()).collect();
    let close: String = stack.iter().rev().map(|(_, s)| s.as_str()).collect();
    // Excessively deep model output is delivered as text rather than exceeding
    // the transport limit with markup alone.
    let (open, close) = if open.len() + close.len() > 1000 {
        (String::new(), String::new())
    } else {
        (open, close)
    };
    let mut run = String::new();
    for ch in text.chars() {
        let encoded = escape(&ch.to_string());
        if html.len() + open.len() + run.len() + encoded.len() + close.len() > 3800 {
            if !run.is_empty() {
                html.push_str(&open);
                html.push_str(&run);
                html.push_str(&close);
                run.clear();
            }
            if !plain.is_empty() {
                parts.push((std::mem::take(html), std::mem::take(plain)));
            }
        }
        run.push_str(&encoded);
        plain.push(ch);
    }
    if !run.is_empty() {
        html.push_str(&open);
        html.push_str(&run);
        html.push_str(&close);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_agent_markdown_and_escapes_html() {
        let parts = reply_parts(
            "# Plan\n\nKeep **working flows**, use `web/remotion/`.\n\n- [repo](https://example.com/a?b=1&c=2)\n\n```rs\nif a < b { x & y }\n```\n<script>bad</script>",
        );
        let html = &parts[0].0;
        assert!(html.contains("<b>Plan</b>"));
        assert!(html.contains("<b>working flows</b>"));
        assert!(html.contains("<code>web/remotion/</code>"));
        assert!(html.contains("href=\"https://example.com/a?b=1&amp;c=2\""));
        assert!(html.contains("<pre>if a &lt; b { x &amp; y }\n</pre>"));
        assert!(!html.contains("<script>"));
        assert!(parts[0].1.contains("• repo"));
    }

    #[test]
    fn splits_long_unicode_code_without_losing_content_or_breaking_tags() {
        let code = "🙂<&>".repeat(2500);
        let parts = reply_parts(&format!("```\n{code}\n```"));
        assert!(parts.len() > 1);
        let plain: String = parts.iter().map(|(_, p)| p.as_str()).collect();
        assert_eq!(plain.trim_end(), code);
        for (html, plain) in parts {
            assert!(html.len() <= 3800);
            assert!(plain.encode_utf16().count() < 4096);
            assert_eq!(
                html.matches("<pre>").count(),
                html.matches("</pre>").count()
            );
            assert!(!html.ends_with("&am"));
        }
    }

    #[test]
    fn malformed_markdown_is_still_delivered_and_unsafe_links_are_not_html() {
        let parts = reply_parts("Unclosed **bold and `code\n\n[x](javascript:alert) <&");
        assert!(parts[0].1.contains("Unclosed **bold"));
        assert!(!parts[0].0.contains("href="));
        assert!(parts[0].0.contains("&lt;&amp;"));
    }
}
