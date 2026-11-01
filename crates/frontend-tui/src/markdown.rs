//! The Markdown an assistant's answer uses, for a terminal: headings, fences, tables, lists,
//! quotes, rules, and bold, italic, strike, inline code and links inside them. What is not markup
//! stays as written. An answer arrives line by line, so a table that has only its header so far
//! reads as text until its separator comes.

/// What a line of an answer is.
#[derive(Debug, PartialEq, Eq)]
pub enum Block<'a> {
    /// A ``` line: not drawn.
    Fence,
    Code(&'a str),
    Heading(&'a str),
    Text(&'a str),
}

pub fn classify<'a>(line: &'a str, in_fence: &mut bool) -> Block<'a> {
    if line.trim_start().starts_with("```") {
        *in_fence = !*in_fence;
        return Block::Fence;
    }
    if *in_fence {
        return Block::Code(line);
    }
    let hashes = line.chars().take_while(|c| *c == '#').count();
    if (1..=6).contains(&hashes) && line[hashes..].starts_with(' ') {
        return Block::Heading(line[hashes..].trim());
    }
    Block::Text(line)
}

/// How a table column lines up, from its separator cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

/// A block of an answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node {
    Code(String),
    Heading(String),
    Text(String),
    Blank,
    Rule,
    Quote(String),
    /// A list item: its nesting depth, what is drawn before it, its text.
    Item { depth: u16, marker: String, text: String },
    Table { aligns: Vec<Align>, header: Vec<String>, rows: Vec<Vec<String>> },
}

/// A table row's cells. A pipe inside inline code or after a backslash stays in its cell.
pub fn cells(line: &str) -> Vec<String> {
    let line = line.trim();
    let line = line.strip_prefix('|').unwrap_or(line);
    let (mut out, mut cell, mut code, mut escaped) = (Vec::new(), String::new(), false, false);
    for c in line.chars() {
        match c {
            _ if escaped => {
                cell.push(c);
                escaped = false;
            }
            '\\' => escaped = true,
            '`' => {
                code = !code;
                cell.push(c);
            }
            '|' if !code => out.push(std::mem::take(&mut cell).trim().to_string()),
            _ => cell.push(c),
        }
    }
    if !cell.trim().is_empty() {
        out.push(cell.trim().to_string());
    }
    out
}

/// The alignments if this line is a table's separator: `|---|:--:|`.
fn separator(line: &str) -> Option<Vec<Align>> {
    if !line.contains('-') || !line.contains('|') {
        return None;
    }
    let parts = cells(line);
    let is_rule = |c: &String| !c.is_empty() && c.contains('-') && c.chars().all(|x| x == '-' || x == ':');
    if parts.is_empty() || !parts.iter().all(is_rule) {
        return None;
    }
    Some(parts.iter().map(|c| match (c.starts_with(':'), c.ends_with(':')) {
        (true, true) => Align::Center,
        (false, true) => Align::Right,
        _ => Align::Left,
    }).collect())
}

/// A list item's parts, from a line: depth, marker, text.
fn list_item(line: &str) -> Option<(u16, String, String)> {
    let rest = line.trim_start();
    let depth = ((line.len() - rest.len()) / 2) as u16;
    for bullet in ["- ", "* ", "+ "] {
        if let Some(text) = rest.strip_prefix(bullet) {
            for (mark, shown) in [("[ ] ", "☐"), ("[x] ", "☑"), ("[X] ", "☑")] {
                if let Some(task) = text.strip_prefix(mark) {
                    return Some((depth, shown.to_string(), task.to_string()));
                }
            }
            return Some((depth, "•".to_string(), text.to_string()));
        }
    }
    let digits = rest.chars().take_while(char::is_ascii_digit).count();
    if digits > 0
        && let Some(text) = rest[digits..].strip_prefix(". ").or(rest[digits..].strip_prefix(") "))
    {
        return Some((depth, format!("{}.", &rest[..digits]), text.to_string()));
    }
    None
}

/// An answer's lines as blocks.
pub fn blocks(lines: &[String]) -> Vec<Node> {
    let mut out: Vec<Node> = Vec::new();
    let mut in_fence = false;
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i].as_str();
        i += 1;
        match classify(line, &mut in_fence) {
            Block::Fence => continue,
            Block::Code(code) => out.push(Node::Code(code.to_string())),
            Block::Heading(text) => out.push(Node::Heading(text.to_string())),
            Block::Text(text) => {
                let trimmed = text.trim();
                if trimmed.is_empty() {
                    // One empty line is a paragraph break; more add nothing.
                    if !matches!(out.last(), Some(Node::Blank) | None) {
                        out.push(Node::Blank);
                    }
                } else if ["---", "***", "___"].iter().any(|r| trimmed.len() >= 3 && trimmed.chars().all(|c| r.starts_with(c))) {
                    out.push(Node::Rule);
                } else if let Some(quote) = trimmed.strip_prefix("> ").or(trimmed.strip_prefix('>')) {
                    out.push(Node::Quote(quote.to_string()));
                } else if trimmed.contains('|') && lines.get(i).is_some_and(|next| separator(next).is_some_and(|s| s.len() == cells(trimmed).len())) {
                    let aligns = separator(&lines[i]).unwrap_or_default();
                    i += 1;
                    let mut rows = Vec::new();
                    while let Some(row) = lines.get(i).filter(|l| l.contains('|') && !l.trim().is_empty()) {
                        rows.push(cells(row));
                        i += 1;
                    }
                    out.push(Node::Table { aligns, header: cells(trimmed), rows });
                } else if let Some((depth, marker, text)) = list_item(text) {
                    out.push(Node::Item { depth, marker, text });
                } else {
                    out.push(Node::Text(text.to_string()));
                }
            }
        }
    }
    while matches!(out.last(), Some(Node::Blank)) {
        out.pop();
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Inline {
    Plain,
    Bold,
    Italic,
    Strike,
    Code,
    Link,
    /// A link's address, after its text.
    Muted,
}

/// The characters of a text with their styles, its markers gone.
pub fn inline(text: &str) -> Vec<(char, Inline)> {
    let chars: Vec<char> = text.chars().collect();
    let closing = |from: usize, mark: &[char]| -> Option<usize> { (from..=chars.len().checked_sub(mark.len())?).find(|&i| chars[i..i + mark.len()] == *mark) };
    let mut out = Vec::with_capacity(chars.len());
    let mut i = 0;
    let wrapped = |out: &mut Vec<(char, Inline)>, from: usize, to: usize, style: Inline| out.extend(chars[from..to].iter().map(|c| (*c, style)));
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        if c == '`'
            && let Some(end) = closing(i + 1, &['`']).filter(|e| *e > i + 1)
        {
            wrapped(&mut out, i + 1, end, Inline::Code);
            i = end + 1;
        } else if c == '*'
            && next == Some('*')
            && (i == 0 || !chars[i - 1].is_alphanumeric())
            && chars.get(i + 2).is_some_and(|n| !n.is_whitespace())
            && let Some(end) = closing(i + 2, &['*', '*']).filter(|e| *e > i + 2)
        {
            wrapped(&mut out, i + 2, end, Inline::Bold);
            i = end + 2;
        } else if c == '_'
            && next == Some('_')
            && (i == 0 || !chars[i - 1].is_alphanumeric())
            && chars.get(i + 2).is_some_and(|n| !n.is_whitespace())
            && let Some(end) = closing(i + 2, &['_', '_']).filter(|e| *e > i + 2 && chars.get(*e + 2).is_none_or(|n| !n.is_alphanumeric()))
        {
            wrapped(&mut out, i + 2, end, Inline::Bold);
            i = end + 2;
        } else if c == '*' && next == Some('*') {
            // Two stars that open nothing, as in 2**3: both stay.
            out.extend([('*', Inline::Plain), ('*', Inline::Plain)]);
            i += 2;
        } else if c == '~'
            && next == Some('~')
            && let Some(end) = closing(i + 2, &['~', '~']).filter(|e| *e > i + 2)
        {
            wrapped(&mut out, i + 2, end, Inline::Strike);
            i = end + 2;
        } else if c == '*'
            && next.is_some_and(|n| !n.is_whitespace() && n != '*')
            && (i == 0 || !chars[i - 1].is_alphanumeric())
            && let Some(end) = closing(i + 1, &['*']).filter(|e| !chars[*e - 1].is_whitespace())
        {
            wrapped(&mut out, i + 1, end, Inline::Italic);
            i = end + 1;
        } else if c == '['
            && let Some(close) = closing(i + 1, &[']', '('])
            && let Some(end) = closing(close + 2, &[')'])
        {
            wrapped(&mut out, i + 1, close, Inline::Link);
            out.extend(" (".chars().map(|c| (c, Inline::Muted)));
            wrapped(&mut out, close + 2, end, Inline::Muted);
            out.push((')', Inline::Muted));
            i = end + 1;
        } else {
            out.push((c, Inline::Plain));
            i += 1;
        }
    }
    out
}

/// How many cells a character takes: an emoji two, a Cyrillic letter one.
pub fn cell_width(c: char) -> usize {
    unicode_width::UnicodeWidthChar::width(c).unwrap_or(0)
}

/// How many cells styled text takes.
pub fn display_width(chars: &[(char, Inline)]) -> usize {
    chars.iter().map(|(c, _)| cell_width(*c)).sum()
}

/// Styled text wrapped to `width` cells: at spaces where it can, inside a word where it must.
pub fn wrap_styled(chars: &[(char, Inline)], width: usize) -> Vec<Vec<(char, Inline)>> {
    let width = width.max(1);
    let mut rows = Vec::new();
    let mut row: Vec<(char, Inline)> = Vec::new();
    let mut used = 0;
    let mut last_space = None;
    for &(c, s) in chars {
        let w = cell_width(c);
        if used + w > width && !row.is_empty() {
            // The space that would overflow is the break itself.
            if c.is_whitespace() {
                rows.push(std::mem::take(&mut row));
                used = 0;
                last_space = None;
                continue;
            }
            match last_space {
                Some(at) => {
                    let rest = row.split_off(at + 1);
                    row.pop();
                    rows.push(std::mem::replace(&mut row, rest));
                }
                None => rows.push(std::mem::take(&mut row)),
            }
            used = display_width(&row);
            last_space = None;
        }
        if c.is_whitespace() {
            last_space = Some(row.len());
        }
        row.push((c, s));
        used += w;
    }
    if !row.is_empty() || rows.is_empty() {
        rows.push(row);
    }
    rows
}

/// Column widths for a table within `room` cells: what each wants if that fits; otherwise the
/// narrow columns keep theirs and the wide ones share what is left.
pub fn column_widths(wants: &[usize], room: usize) -> Vec<usize> {
    if wants.iter().sum::<usize>() <= room {
        return wants.to_vec();
    }
    let mut out = vec![0; wants.len()];
    let mut left = room;
    let mut open: Vec<usize> = (0..wants.len()).collect();
    loop {
        let share = left / open.len().max(1);
        let (fits, wide): (Vec<usize>, Vec<usize>) = open.iter().partition(|&&i| wants[i] <= share);
        if fits.is_empty() {
            let mut extra = left.saturating_sub(share * wide.len());
            for i in wide {
                out[i] = share.max(1) + usize::from(extra > 0);
                extra = extra.saturating_sub(1);
            }
            return out;
        }
        for i in fits {
            out[i] = wants[i];
            left -= wants[i];
        }
        open = wide;
        if open.is_empty() {
            return out;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shown(text: &str) -> (String, Vec<Inline>) {
        let v = inline(text);
        (v.iter().map(|(c, _)| *c).collect(), v.iter().map(|(_, s)| *s).collect())
    }

    fn styled(text: &str, style: Inline) -> String {
        inline(text).iter().filter(|(_, s)| *s == style).map(|(c, _)| *c).collect()
    }

    #[test]
    fn bold_and_code_lose_their_markers() {
        assert_eq!(shown("17 × 23 = **391**.").0, "17 × 23 = 391.");
        assert_eq!(styled("17 × 23 = **391**.", Inline::Bold), "391");
        assert_eq!(styled("run `cargo test` now", Inline::Code), "cargo test");
        assert_eq!(styled("__both__ forms", Inline::Bold), "both");
    }

    #[test]
    fn what_is_not_markup_stays_as_written() {
        for text in ["2**3 and 2**4", "snake__case__name", "an **unclosed marker", "a ` lone tick", "2 * 3 * 4", "** spaced **", "``"] {
            assert_eq!(shown(text).0, text, "{text}");
        }
        assert_eq!(styled("snake_case and _one_", Inline::Bold), "");
    }

    #[test]
    fn code_inside_keeps_its_stars() {
        assert_eq!(styled("`**not bold**` here", Inline::Code), "**not bold**");
        assert_eq!(styled("`**not bold**` here", Inline::Bold), "");
    }

    #[test]
    fn fences_and_headings_are_lines_of_their_own() {
        let mut fence = false;
        assert_eq!(classify("## Plan", &mut fence), Block::Heading("Plan"));
        assert_eq!(classify("#not a heading", &mut fence), Block::Text("#not a heading"));
        assert_eq!(classify("```rust", &mut fence), Block::Fence);
        assert_eq!(classify("# a comment in code", &mut fence), Block::Code("# a comment in code"));
        assert_eq!(classify("```", &mut fence), Block::Fence);
        assert_eq!(classify("after", &mut fence), Block::Text("after"));
    }

    fn lines(text: &str) -> Vec<String> {
        text.lines().map(str::to_string).collect()
    }

    #[test]
    fn a_table_is_found_with_its_cells_and_alignment() {
        let nodes = blocks(&lines("Before\n\n| Group | Tools |\n|---|:--:|\n| Files | `a|b`, `c` |\n| HTTP | \\| x |\n\nAfter"));
        assert_eq!(nodes[0], Node::Text("Before".into()));
        let Node::Table { aligns, header, rows } = &nodes[2] else { panic!("{nodes:?}") };
        assert_eq!(aligns, &[Align::Left, Align::Center]);
        assert_eq!(header, &["Group", "Tools"]);
        assert_eq!(rows[0], ["Files", "`a|b`, `c`"], "a pipe in code stays");
        assert_eq!(rows[1], ["HTTP", "| x"], "an escaped pipe stays");
        assert_eq!(nodes[4], Node::Text("After".into()));
    }

    #[test]
    fn half_a_table_reads_as_text_and_a_fenced_one_stays_code() {
        assert_eq!(blocks(&lines("| a | b |")), [Node::Text("| a | b |".into())], "its separator has not come yet");
        assert_eq!(blocks(&lines("```\n| a |\n|---|\n```")), [Node::Code("| a |".into()), Node::Code("|---|".into())]);
        assert_eq!(blocks(&lines("a | b is not a table\nnext")), [Node::Text("a | b is not a table".into()), Node::Text("next".into())]);
    }

    #[test]
    fn lists_quotes_and_rules() {
        let nodes = blocks(&lines("- one\n  * two\n1. three\n- [x] done\n- [ ] todo\n> said\n---\n\n\n\nend\n\n"));
        let item = |depth, marker: &str, text: &str| Node::Item { depth, marker: marker.into(), text: text.into() };
        assert_eq!(nodes, [
            item(0, "•", "one"), item(1, "•", "two"), item(0, "1.", "three"), item(0, "☑", "done"), item(0, "☐", "todo"),
            Node::Quote("said".into()), Node::Rule, Node::Blank, Node::Text("end".into()),
        ]);
    }

    #[test]
    fn the_other_inline_styles() {
        assert_eq!(styled("a *it* b", Inline::Italic), "it");
        assert_eq!(styled("~~gone~~ x", Inline::Strike), "gone");
        assert_eq!(styled("see [docs](http://x.y) now", Inline::Link), "docs");
        assert_eq!(shown("see [docs](http://x.y) now").0, "see docs (http://x.y) now");
        assert_eq!(shown("2 * 3 * 4 and a*b*c").0, "2 * 3 * 4 and a*b*c", "not italics");
        assert_eq!(shown("[not a link] (x)").0, "[not a link] (x)");
    }

    #[test]
    fn wrapping_counts_cells_and_breaks_at_spaces() {
        let plain = |t: &str| inline(t);
        let text = |rows: Vec<Vec<(char, Inline)>>| rows.into_iter().map(|r| r.into_iter().map(|(c, _)| c).collect::<String>()).collect::<Vec<_>>();
        assert_eq!(text(wrap_styled(&plain("one two three"), 7)), ["one two", "three"]);
        assert_eq!(text(wrap_styled(&plain("abcdefgh"), 3)), ["abc", "def", "gh"]);
        assert_eq!(text(wrap_styled(&plain("✅ ok да"), 5)), ["✅ ok", "да"], "an emoji takes two cells");
        assert_eq!(display_width(&plain("✅да")), 4);
        assert_eq!(text(wrap_styled(&plain(""), 5)), [""]);
    }

    #[test]
    fn narrow_columns_keep_their_width_and_wide_ones_share() {
        assert_eq!(column_widths(&[5, 10], 20), [5, 10]);
        assert_eq!(column_widths(&[5, 50, 50], 45), [5, 20, 20]);
        assert_eq!(column_widths(&[50, 50], 21).iter().sum::<usize>(), 21);
    }
}
