//! Selecting text in the window (CON-188). The window takes the mouse for its clicks, tabs and
//! maps, so it selects text itself: on the frame it drew, in screen cells, in reading order as a
//! terminal does. The text of a selection is read from that frame, so it is the same for the
//! transcript, its cards and the composer.

use ratatui::buffer::Buffer;
use ratatui::style::Modifier;

/// A selection from where the button was pressed (`anchor`) to where the pointer is (`head`),
/// in screen cells (column, row).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    pub anchor: (u16, u16),
    pub head: (u16, u16),
}

impl Selection {
    /// Start and end in reading order: the earlier row first, or the earlier column on one row.
    pub fn ordered(&self) -> ((u16, u16), (u16, u16)) {
        let (a, h) = (self.anchor, self.head);
        if (a.1, a.0) <= (h.1, h.0) { (a, h) } else { (h, a) }
    }

    /// Whether the cell is in the selection: the first row from the start, the last row up to
    /// the end, every row between whole.
    pub fn contains(&self, col: u16, row: u16) -> bool {
        let ((sc, sr), (ec, er)) = self.ordered();
        if row < sr || row > er {
            return false;
        }
        (row > sr || col >= sc) && (row < er || col <= ec)
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }
}

/// The window's own drawing, not text: box lines (a card's border, a toast's frame) and blocks
/// (a message's bar).
fn decoration(c: char) -> bool {
    ('\u{2500}'..='\u{259F}').contains(&c)
}

/// A row of a selection of several rows, without the window's decorations at its edges: the
/// message's bar and the spaces after it, and from a border that stands apart from the text (three
/// spaces or more before it) to the end, such as a card's right edge or a toast beside.
fn undecorated(row: &str) -> String {
    let start = row.trim_start_matches(|c: char| decoration(c) || c == ' ');
    let mut end = start.len();
    let mut spaces = 0;
    for (i, c) in start.char_indices() {
        if c == ' ' {
            spaces += 1;
        } else {
            if decoration(c) && spaces >= 3 {
                end = i;
                break;
            }
            spaces = 0;
        }
    }
    start[..end].trim_end().to_string()
}

/// The selected text of a frame: rows joined by line breaks, each row's trailing spaces dropped.
/// A wide character is taken once, from the cell that holds it. A selection of several rows
/// leaves out the window's decorations at each row's edges.
pub fn text(buf: &Buffer, sel: &Selection) -> String {
    let area = buf.area;
    let ((_, sr), (_, er)) = sel.ordered();
    let several = er > sr;
    let mut rows = Vec::new();
    for row in sr.max(area.y)..=er.min(area.bottom().saturating_sub(1)) {
        let mut line = String::new();
        let mut col = area.x;
        while col < area.right() {
            let symbol = buf[(col, row)].symbol();
            // A wide character takes its cell and the next: the next is not read again.
            let width = unicode_width::UnicodeWidthStr::width(symbol).max(1) as u16;
            if sel.contains(col, row) {
                line.push_str(symbol);
            }
            col += width;
        }
        rows.push(if several { undecorated(&line) } else { line.trim_end().to_string() });
    }
    rows.join("\n")
}

/// Draws the selection over a frame: its cells reversed.
pub fn highlight(buf: &mut Buffer, sel: &Selection) {
    let area = buf.area;
    let ((_, sr), (_, er)) = sel.ordered();
    for row in sr.max(area.y)..=er.min(area.bottom().saturating_sub(1)) {
        for col in area.x..area.right() {
            if sel.contains(col, row) {
                let cell = &mut buf[(col, row)];
                cell.set_style(cell.style().add_modifier(Modifier::REVERSED));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;

    fn frame(lines: &[&str]) -> Buffer {
        let w = lines.iter().map(|l| unicode_width::UnicodeWidthStr::width(*l)).max().unwrap_or(0) as u16;
        let mut buf = Buffer::empty(Rect::new(0, 0, w.max(1), lines.len() as u16));
        for (y, l) in lines.iter().enumerate() {
            buf.set_string(0, y as u16, l, ratatui::style::Style::default());
        }
        buf
    }

    #[test]
    fn one_row_from_press_to_pointer_either_way() {
        let buf = frame(&["hello world   "]);
        let s = Selection { anchor: (6, 0), head: (10, 0) };
        assert_eq!(text(&buf, &s), "world");
        let back = Selection { anchor: (10, 0), head: (6, 0) };
        assert_eq!(text(&buf, &back), "world", "dragged to the left");
    }

    #[test]
    fn several_rows_in_reading_order_with_trailing_spaces_dropped() {
        let buf = frame(&["first line    ", "second line   ", "third line    "]);
        let s = Selection { anchor: (6, 0), head: (4, 2) };
        assert_eq!(text(&buf, &s), "line\nsecond line\nthird");
        let up = Selection { anchor: (4, 2), head: (6, 0) };
        assert_eq!(text(&buf, &up), "line\nsecond line\nthird", "dragged upwards");
    }

    #[test]
    fn several_rows_leave_out_the_windows_decorations() {
        let buf = frame(&["▌ first row of a message      ", "▌ second row         │ Copied 3 │", "│ in a card │                    "]);
        let s = Selection { anchor: (2, 0), head: (11, 2) };
        assert_eq!(text(&buf, &s), "first row of a message\nsecond row\nin a card");
        let one = Selection { anchor: (0, 1), head: (12, 1) };
        assert_eq!(text(&buf, &one), "▌ second row", "one row is taken as it is");
        let table = frame(&["a │ b │ c ", "d │ e │ f "]);
        assert_eq!(text(&table, &Selection { anchor: (0, 0), head: (8, 1) }), "a │ b │ c\nd │ e │ f", "a table's own lines stay");
    }

    #[test]
    fn a_wide_character_is_taken_once() {
        let buf = frame(&["ab日本cd"]);
        let s = Selection { anchor: (0, 0), head: (7, 0) };
        assert_eq!(text(&buf, &s), "ab日本cd");
    }

    #[test]
    fn the_highlight_reverses_exactly_the_selected_cells() {
        let mut buf = frame(&["abcd", "efgh"]);
        let s = Selection { anchor: (2, 0), head: (1, 1) };
        highlight(&mut buf, &s);
        let reversed: Vec<(u16, u16)> = (0..2).flat_map(|r| (0..4).map(move |c| (c, r))).filter(|&(c, r)| buf[(c, r)].style().add_modifier.contains(Modifier::REVERSED)).collect();
        assert_eq!(reversed, [(2, 0), (3, 0), (0, 1), (1, 1)]);
    }
}
