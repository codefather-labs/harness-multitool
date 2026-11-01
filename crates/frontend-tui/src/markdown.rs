//! The little Markdown an assistant's answer needs in a terminal: bold, inline code, headings
//! and fenced code. The rest stays as written, which reads fine in a monospace font.

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Inline {
    Plain,
    Bold,
    Code,
}

/// The characters of a line with their styles, markers removed. A marker with no closing pair
/// stays literal, and `**` or `__` inside a word (snake__case) is not a marker.
pub fn inline(text: &str) -> Vec<(char, Inline)> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::with_capacity(chars.len());
    let mut i = 0;
    let find = |from: usize, pat: &[char]| (from..chars.len().saturating_sub(pat.len() - 1)).find(|&j| chars[j..j + pat.len()] == *pat);
    while i < chars.len() {
        let c = chars[i];
        if c == '`' {
            if let Some(end) = find(i + 1, &['`']).filter(|e| *e > i + 1) {
                out.extend(chars[i + 1..end].iter().map(|c| (*c, Inline::Code)));
                i = end + 1;
                continue;
            }
        } else if (c == '*' || c == '_') && chars.get(i + 1) == Some(&c) {
            let word_before = i > 0 && chars[i - 1].is_alphanumeric();
            let opens = chars.get(i + 2).is_some_and(|n| !n.is_whitespace());
            if opens
                && !word_before
                && let Some(end) = find(i + 2, &[c, c]).filter(|e| *e > i + 2 && !chars[*e - 1].is_whitespace())
            {
                out.extend(chars[i + 2..end].iter().map(|c| (*c, Inline::Bold)));
                i = end + 2;
                continue;
            }
        }
        out.push((c, Inline::Plain));
        i += 1;
    }
    out
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
}
