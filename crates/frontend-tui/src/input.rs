//! What keys and clicks do to the view model. The library decides the local effect (move a
//! selection, edit the composer) and reports what the app must do next (open a session, leave
//! the splash); the app fetches the data for the next screen, the view model never does.

use crate::model::{AppState, Mode, StartupPanel};
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
    Char(char),
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
    /// The composer's text was sent.
    Send,
}

pub fn key(s: &mut AppState, k: Key) -> Outcome {
    if k == Key::Esc {
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
        Mode::Session(v) => {
            match k {
                Key::Enter => {
                    v.composer.text.clear();
                    return Outcome::Send;
                }
                Key::Backspace => {
                    v.composer.text.pop();
                }
                Key::Char(c) => v.composer.text.push(c),
                _ => {}
            }
            Outcome::Stay
        }
    }
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
        (Mode::Session(_), Target::Settings) => Outcome::OpenSettings,
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
}
