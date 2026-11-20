//! The terminal frontend (IF-11, CON-6), drawn with ratatui.
//!
//! `model::AppState` is the view model; `view::draw` is the one drawing entry point. The live CLI
//! and the snapshot tool both use them, so the named states in `fixtures` are real CLI screens
//! (docs/design/concept holds the mockups they are checked against).

pub mod debug;
pub mod fixtures;
pub mod flowchart;
pub mod home;
pub mod images;
pub mod maps;
pub mod markdown;
pub mod mindmap;
pub mod input;
pub mod live;
pub mod model;
pub mod overlay;
pub mod selection;
pub mod theme;
pub mod transcript;
pub mod view;
pub mod widgets;

pub use model::AppState;
pub use theme::Theme;
pub use view::draw;
