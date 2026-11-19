//! Source offsets on rendered blocks for semantic split-view navigation.
use pulldown_cmark::{CowStr, Event, Parser, Tag};

pub fn events<'a>(
    parser: Parser<'a>,
    markdown: &str,
    document: &str,
    base: usize,
) -> Vec<Event<'a>> {
    let mut offsets = vec![0; document.len() + 1];
    let mut units = 0;
    for (index, ch) in document.char_indices() {
        offsets[index] = units;
        if ch != '\r' || document.as_bytes().get(index + 1) != Some(&b'\n') {
            units += ch.len_utf16();
        }
    }
    offsets[document.len()] = units;
    let mut depth = 0;
    let mut result = Vec::new();
    for (event, range) in parser.into_offset_iter() {
        let mapped = match &event {
            Event::Start(Tag::Paragraph) => depth == 0,
            Event::Start(
                Tag::Heading { .. }
                | Tag::CodeBlock(_)
                | Tag::Item
                | Tag::Table(_)
                | Tag::TableRow
                | Tag::BlockQuote(_),
            ) => true,
            Event::Rule => true,
            _ => false,
        };
        if mapped {
            debug_assert!(range.end <= markdown.len());
            result.push(Event::Html(CowStr::from(format!(
                "<!--mdp-pos:{}:{}-->",
                offsets[base + range.start],
                offsets[base + range.end]
            ))));
        }
        match &event {
            Event::Start(_) => depth += 1,
            Event::End(_) => depth -= 1,
            _ => {}
        }
        result.push(event);
    }
    result
}

pub fn attach(html: String) -> String {
    let pattern = regex::Regex::new(r"<!--mdp-pos:(\d+):(\d+)-->\s*<([a-z][a-z0-9]*)").unwrap();
    pattern
        .replace_all(&html, r#"<$3 data-source-start="$1" data-source-end="$2""#)
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulldown_cmark::{html, Options};
    #[test]
    fn maps_cjk_emoji_crlf_and_metadata_to_textarea_offsets() {
        let document = "---\r\na: 1\r\n---\r\n# 中文🙂\r\n\r\n正文\r\n";
        let body = "# 中文🙂\r\n\r\n正文\r\n";
        let parser = Parser::new_ext(body, Options::empty());
        let mut output = String::new();
        html::push_html(
            &mut output,
            events(parser, body, document, document.len() - body.len()).into_iter(),
        );
        let output = attach(output);
        assert!(output.contains("<h1 data-source-start=\"13\""), "{output}");
        assert!(output.contains("<p data-source-start=\"21\""), "{output}");
        assert!(!output.contains("<!--mdp-pos:"));
    }
    #[test]
    fn annotates_list_items_and_rows_without_extra_dom_wrappers() {
        let source = "- first\n- second\n\n| A | B |\n|---|---|\n| c | d |\n";
        let mut output = String::new();
        html::push_html(
            &mut output,
            events(
                Parser::new_ext(source, Options::ENABLE_TABLES),
                source,
                source,
                0,
            )
            .into_iter(),
        );
        let output = attach(output);
        assert_eq!(output.matches("<li data-source-start=").count(), 2);
        assert!(output.contains("<tr data-source-start="));
        assert!(!output.contains("<div"));
    }
}
