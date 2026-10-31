//! The view model the terminal frontend draws. The live CLI keeps one `AppState` and updates it
//! from core events and input; the snapshot tool builds one from a fixture. Both draw it with
//! `view::draw`, so a fixture screen is exactly what the CLI shows in that state.

/// Everything on screen.
#[derive(Debug, Clone)]
pub struct AppState {
    pub header: HeaderInfo,
    pub mode: Mode,
}

#[derive(Debug, Clone)]
pub struct HeaderInfo {
    pub product: String,
    pub version: String,
    /// What follows the product name: the version on the welcome screen, `session/<id>` in a session.
    pub location: String,
    pub cwd: String,
    /// `adapter / model`, shown when known.
    pub adapter_model: Option<String>,
    pub branch: Option<String>,
    pub online: bool,
}

#[derive(Debug, Clone)]
pub enum Mode {
    Session(SessionView),
}

#[derive(Debug, Clone, Default)]
pub struct SessionView {
    pub transcript: Vec<Item>,
    pub composer: Composer,
    pub hints: Vec<KeyHint>,
}

/// One entry of the transcript.
#[derive(Debug, Clone)]
pub enum Item {
    /// The card a fresh session opens with.
    Welcome { title: String, tagline: String },
    /// A card of tips: a label and bullet lines.
    Tips { label: String, lines: Vec<String> },
}

#[derive(Debug, Clone, Default)]
pub struct Composer {
    pub text: String,
    pub placeholder: String,
    pub focused: bool,
}

#[derive(Debug, Clone)]
pub struct KeyHint {
    pub key: String,
    pub label: String,
}

impl KeyHint {
    pub fn new(key: &str, label: &str) -> KeyHint {
        KeyHint { key: key.into(), label: label.into() }
    }
}
