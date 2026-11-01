//! What keys and clicks do to the view model. The library decides the local effect (move a
//! selection, edit the composer) and reports what the app must do next (open a session, leave
//! the splash); the app fetches the data for the next screen, the view model never does.

use crate::model::{AppState, Mode, StartupPanel, TabMark, Workspace};
use crate::view::{Drawn, Target};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Enter,
    Esc,
    Up,
    Down,
    Left,
    Right,
    Tab,
    Backspace,
    PageUp,
    PageDown,
    Home,
    End,
    Char(char),
    /// A letter with Ctrl held.
    Ctrl(char),
    /// A key with Alt held.
    Alt(char),
}

/// What the app has to do after an input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Handled inside the view model (or ignored).
    Stay,
    Quit,
    /// The splash was confirmed: show the startup page.
    LeaveSplash,
    /// Open recent session `i` of the startup page.
    OpenSession(usize),
    /// Run quick action `i` of the startup page.
    Action(usize),
    OpenSettings,
}

pub fn key(s: &mut AppState, k: Key) -> Outcome {
    if k == Key::Esc && !matches!(s.mode, Mode::Session(_)) {
        return Outcome::Quit;
    }
    match &mut s.mode {
        Mode::Splash(_) => match k {
            Key::Enter | Key::Char(' ') => Outcome::LeaveSplash,
            _ => Outcome::Stay,
        },
        Mode::Startup(v) => {
            let (len, sel) = match v.focus {
                StartupPanel::Sessions => (v.sessions.len(), &mut v.selected_session),
                StartupPanel::Actions => (v.actions.len(), &mut v.selected_action),
            };
            match k {
                Key::Up if *sel > 0 => *sel -= 1,
                Key::Down if *sel + 1 < len => *sel += 1,
                Key::Tab | Key::Left | Key::Right => {
                    v.focus = match v.focus {
                        StartupPanel::Sessions => StartupPanel::Actions,
                        StartupPanel::Actions => StartupPanel::Sessions,
                    }
                }
                Key::Enter => {
                    return match v.focus {
                        StartupPanel::Sessions => Outcome::OpenSession(v.selected_session),
                        StartupPanel::Actions => Outcome::Action(v.selected_action),
                    };
                }
                Key::Char(c) => {
                    if let Some(i) = v.actions.iter().position(|a| a.key == c) {
                        v.focus = StartupPanel::Actions;
                        v.selected_action = i;
                        return Outcome::Action(i);
                    }
                }
                _ => {}
            }
            Outcome::Stay
        }
        Mode::Session(w) => {
            if let Some(o) = crate::overlay::key(w, k) {
                return o;
            }
            match k {
                Key::Enter => {
                    if crate::debug::submit(w) == crate::debug::Sent::Quit {
                        return Outcome::Quit;
                    }
                }
                Key::Backspace => {
                    w.view_mut().composer.text.pop();
                }
                Key::Char(c) => w.view_mut().composer.text.push(c),
                // Esc closes the toast; the session itself is left with Ctrl+C.
                Key::Esc => w.toast = None,
                Key::PageUp => scroll_view(w, 10),
                Key::PageDown => scroll_view(w, -10),
                Key::Home => scroll_view(w, 100_000),
                Key::End => {
                    w.view_mut().scroll = 0;
                }
                Key::Ctrl('t') => {
                    let title = format!("session-{}", w.tabs.len() + 1);
                    crate::debug::open_tab(w, &title);
                }
                Key::Ctrl('w') => close_tab(w),
                Key::Alt(c) if c.is_ascii_digit() && c != '0' => switch_tab(w, c as usize - '1' as usize),
                _ => {}
            }
            Outcome::Stay
        }
    }
}

/// Unfolds the journal entries of a map part beside its map, or folds them when that part is
/// already open (journal#262).
fn toggle_map_part(w: &mut Workspace, item: usize, sub: Option<usize>, element: crate::model::MapElement) {
    use crate::model::Item;
    let Some(it) = w.view_mut().transcript.get_mut(item) else { return };
    let target = match (it, sub) {
        (Item::Turn { attached, .. }, Some(i)) => attached.get_mut(i),
        (it, None) => Some(it),
        _ => None,
    };
    let open = match target {
        Some(Item::MindMap(e)) => &mut e.open,
        Some(Item::Flowchart(e)) => &mut e.open,
        _ => return,
    };
    *open = if *open == Some(element) { None } else { Some(element) };
}

/// Shows tab `i`; its unread mark is cleared because its messages are now in view.
fn switch_tab(w: &mut Workspace, i: usize) {
    if i < w.tabs.len() {
        w.active = i;
        w.tabs[i].mark = TabMark::None;
    }
}

/// Closes the active tab; the last tab stays (closing it would leave no session).
fn close_tab(w: &mut Workspace) {
    if w.tabs.len() > 1 {
        w.tabs.remove(w.active);
        w.active = w.active.min(w.tabs.len() - 1);
    }
}

/// Scrolls the active transcript by `rows` (positive: back in time). The view clamps it to
/// the transcript's height when drawing.
pub fn scroll(s: &mut AppState, rows: i32) {
    if let Mode::Session(w) = &mut s.mode {
        scroll_view(w, rows);
    }
}

fn scroll_view(w: &mut Workspace, rows: i32) {
    let v = w.view_mut();
    v.scroll = (v.scroll as i64 + rows as i64).clamp(0, 100_000) as usize;
}

/// A left click at a cell, against what the last draw reported.
pub fn click(s: &mut AppState, drawn: &Drawn, col: u16, row: u16) -> Outcome {
    let Some(target) = drawn.hit(col, row) else { return Outcome::Stay };
    match (&mut s.mode, target) {
        (Mode::Splash(_), Target::SplashOk) => Outcome::LeaveSplash,
        (Mode::Startup(v), Target::Session(i)) => {
            v.focus = StartupPanel::Sessions;
            v.selected_session = i;
            Outcome::OpenSession(i)
        }
        (Mode::Startup(v), Target::Action(i)) => {
            v.focus = StartupPanel::Actions;
            v.selected_action = i;
            Outcome::Action(i)
        }
        (Mode::Session(w), t) if w.overlay.is_some() => crate::overlay::click(w, t),
        (Mode::Session(w), Target::MapPart { item, sub, element }) => {
            toggle_map_part(w, item, sub, element);
            Outcome::Stay
        }
        (Mode::Session(w), Target::Settings) => {
            w.overlay = Some(crate::model::Overlay::Settings(crate::overlay::sample_settings()));
            Outcome::Stay
        }
        (Mode::Session(w), Target::Tab(i)) => {
            switch_tab(w, i);
            Outcome::Stay
        }
        (Mode::Session(w), Target::NewTab) => {
            let title = format!("session-{}", w.tabs.len() + 1);
            crate::debug::open_tab(w, &title);
            Outcome::Stay
        }
        (Mode::Session(w), Target::CloseToast) => {
            w.toast = None;
            Outcome::Stay
        }
        _ => Outcome::Stay,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures;

    #[test]
    fn enter_leaves_the_splash() {
        let mut s = (fixtures::by_name("splash").unwrap().build)();
        assert_eq!(key(&mut s, Key::Enter), Outcome::LeaveSplash);
    }

    #[test]
    fn startup_keys_move_and_choose() {
        let mut s = (fixtures::by_name("startup").unwrap().build)();
        assert_eq!(key(&mut s, Key::Down), Outcome::Stay);
        assert_eq!(key(&mut s, Key::Enter), Outcome::Action(1));
        assert_eq!(key(&mut s, Key::Tab), Outcome::Stay);
        assert_eq!(key(&mut s, Key::Enter), Outcome::OpenSession(0));
        assert_eq!(key(&mut s, Key::Char('n')), Outcome::Action(0));
    }

    #[test]
    fn session_keys_manage_tabs_and_scroll() {
        let mut s = fixtures::welcome();
        let tabs = |s: &AppState| match &s.mode {
            Mode::Session(w) => (w.tabs.len(), w.active),
            _ => unreachable!(),
        };
        key(&mut s, Key::Ctrl('t'));
        assert_eq!(tabs(&s), (2, 1));
        key(&mut s, Key::Alt('1'));
        assert_eq!(tabs(&s), (2, 0));
        key(&mut s, Key::Ctrl('w'));
        assert_eq!(tabs(&s), (1, 0));
        key(&mut s, Key::Ctrl('w'));
        assert_eq!(tabs(&s), (1, 0), "the last tab stays");
        assert_eq!(key(&mut s, Key::Esc), Outcome::Stay, "Esc does not quit a session");
        for c in "/exit".chars() {
            key(&mut s, Key::Char(c));
        }
        assert_eq!(key(&mut s, Key::Enter), Outcome::Quit, "/exit quits");
    }

    #[test]
    fn a_click_on_a_drawn_ref_chip_reaches_its_map() {
        use crate::model::{Item, MapElement};
        use ratatui::{Terminal, backend::TestBackend};
        let t = crate::theme::Theme::builtin("dark").unwrap();
        let mut s = fixtures::maps();
        let mut term = Terminal::new(TestBackend::new(170, 120)).unwrap();
        let mut drawn = Drawn::default();
        term.draw(|f| drawn = crate::draw(f, &s, &t)).unwrap();
        let chip = drawn
            .hits
            .iter()
            .find(|h| matches!(h.target, Target::MapPart { element: MapElement::Entry(137), sub: Some(2), .. }))
            .expect("the #137 chip of the mind map is clickable");
        click(&mut s, &drawn, chip.rect.x, chip.rect.y);
        let open = match &s.mode {
            Mode::Session(w) => match &w.view().transcript[0] {
                Item::Turn { attached, .. } => match &attached[2] {
                    Item::MindMap(e) => e.open,
                    _ => unreachable!(),
                },
                _ => unreachable!(),
            },
            _ => unreachable!(),
        };
        assert_eq!(open, None, "the fixture had #137 open; the click folded it");
    }

    #[test]
    fn clicking_a_journal_ref_unfolds_and_folds_its_ticket() {
        use crate::model::{Item, MapElement};
        let mut s = fixtures::maps();
        let at = |s: &AppState| match &s.mode {
            Mode::Session(w) => match &w.view().transcript[0] {
                Item::Turn { attached, .. } => match &attached[2] {
                    Item::MindMap(e) => e.open,
                    _ => unreachable!(),
                },
                _ => unreachable!(),
            },
            _ => unreachable!(),
        };
        assert_eq!(at(&s), Some(MapElement::Entry(137)), "the fixture shows #137 unfolded");
        if let Mode::Session(w) = &mut s.mode {
            toggle_map_part(w, 0, Some(2), MapElement::Entry(137));
        }
        assert_eq!(at(&s), None, "a second click folds it");
        if let Mode::Session(w) = &mut s.mode {
            toggle_map_part(w, 0, Some(2), MapElement::Node(5));
        }
        assert_eq!(at(&s), Some(MapElement::Node(5)));
        key(&mut s, Key::PageUp);
        if let Mode::Session(w) = &s.mode {
            assert_eq!(w.view().scroll, 10);
        }
        key(&mut s, Key::End);
        if let Mode::Session(w) = &s.mode {
            assert_eq!(w.view().scroll, 0);
        }
    }
}
