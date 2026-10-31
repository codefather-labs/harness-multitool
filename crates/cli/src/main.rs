//! `multitool`: the harness binary. For now it carries the design-debug `screen` command, which
//! renders a named state of the terminal UI to PNG (journal: tui-screens specification).

use std::path::PathBuf;

use anyhow::{Context, Result, anyhow, bail};
use clap::{Parser, Subcommand};
use frontend_tui::{Theme, fixtures};
use tui_snapshot::{Raster, side_by_side};

mod config;

#[derive(Parser)]
#[command(name = "multitool", version, about = "harness-multitool")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Open the terminal UI on a named state (default: welcome). F2 cycles themes; /exit or Ctrl+C quits (docs/tui.md).
    Preview {
        #[arg(default_value = "welcome")]
        name: String,
        #[arg(long, default_value = "dark")]
        theme: String,
    },
    /// Write the user's configuration (config.toml in the OS's config directory).
    Setup {
        /// How pictures (logo, icons, big text) are drawn: auto, sixel, kitty, iterm2 or off.
        #[arg(long, value_enum)]
        graphics: Option<config::Graphics>,
    },
    /// Render the logo to PNG at a given size, coloured for a theme, on its background.
    Logo {
        #[arg(long, default_value = "light")]
        theme: String,
        #[arg(long, default_value_t = 1080)]
        width: u32,
        #[arg(long)]
        png: PathBuf,
    },
    /// Render a named UI state to PNG, as the CLI would show it.
    Screen {
        /// Fixture name; omit with --list.
        name: Option<String>,
        /// List the fixture names.
        #[arg(long)]
        list: bool,
        #[arg(long, default_value = "dark")]
        theme: String,
        /// Grid size in cells, COLSxROWS.
        #[arg(long, default_value = "136x34")]
        size: String,
        /// Output PNG.
        #[arg(long)]
        png: Option<PathBuf>,
        /// Put the reference mockup next to the render (docs/design/concept/<mockup>-<theme>.png).
        #[arg(long)]
        compare: bool,
        /// Show the text fallbacks, as a terminal without graphics does.
        #[arg(long)]
        no_graphics: bool,
        /// Draw the logo with its eyes shut (the blink frame).
        #[arg(long)]
        eyes_closed: bool,
        /// Where the mockups live.
        #[arg(long, default_value = "docs/design/concept")]
        mockups: PathBuf,
        /// Regular face instead of the built-in DejaVu Sans Mono (needs --font-bold too).
        #[arg(long, requires = "font_bold")]
        font: Option<PathBuf>,
        /// Bold face instead of the built-in one.
        #[arg(long, requires = "font")]
        font_bold: Option<PathBuf>,
        /// Font size in pixels; the cell width follows from it.
        #[arg(long, default_value_t = 20.0)]
        px: f32,
        /// Cell height in pixels.
        #[arg(long, default_value_t = 26)]
        cell_h: u32,
    },
}

fn main() -> Result<()> {
    let command = Cli::parse().command.unwrap_or(Command::Preview { name: "welcome".into(), theme: "dark".into() });
    match command {
        Command::Preview { name, theme } => preview(&name, &theme),
        Command::Setup { graphics } => {
            let mut c = config::load()?;
            if let Some(g) = graphics {
                c.terminal.graphics = g;
            }
            let p = config::save(&c)?;
            println!("{}\n{}", p.display(), toml::to_string_pretty(&c)?);
            Ok(())
        }
        Command::Logo { theme, width, png } => {
            let theme = Theme::builtin(&theme)?;
            let height = width * 82 / 530;
            let pic = frontend_tui::images::rasterize(&frontend_tui::images::Graphic::Logo(frontend_tui::images::Eyes::Open), &theme, width, height);
            let bg = match theme.background {
                ratatui::style::Color::Rgb(r, g, b) => image::Rgba([r, g, b, 255]),
                _ => image::Rgba([0, 0, 0, 255]),
            };
            let mut out = image::RgbaImage::from_pixel(width, height, bg);
            image::imageops::overlay(&mut out, &pic, 0, 0);
            out.save(&png)?;
            println!("{} {width}x{height}", png.display());
            Ok(())
        }
        Command::Screen { name, list, theme, size, png, compare, no_graphics, eyes_closed, mockups, font, font_bold, px, cell_h } => {
            if list {
                for f in fixtures::ALL {
                    println!("{}\t{}", f.name, f.mockup);
                }
                return Ok(());
            }
            let name = name.ok_or_else(|| anyhow!("a screen name is required (see --list)"))?;
            let fixture = fixtures::by_name(&name).ok_or_else(|| anyhow!("unknown screen {name:?} (see --list)"))?;
            let theme = Theme::builtin(&theme)?;
            let (cols, rows) = size
                .split_once('x')
                .and_then(|(c, r)| Some((c.parse::<u16>().ok()?, r.parse::<u16>().ok()?)))
                .ok_or_else(|| anyhow!("--size must be COLSxROWS, got {size:?}"))?;
            let Some(out) = png else { bail!("--png <file> is required") };

            let raster = match (&font, &font_bold) {
                (Some(r), Some(b)) => Raster::with_fonts(r, b, px, cell_h)?,
                _ => Raster::new(px, cell_h)?,
            };
            let mut state = (fixture.build)();
            if eyes_closed {
                state.eyes = frontend_tui::images::Eyes::Closed;
            }
            let mut img = raster.render_with(&state, &theme, cols, rows, !no_graphics);
            if compare {
                let path = mockups.join(format!("{}-{}.png", fixture.mockup, theme.id));
                let mockup = image::open(&path).with_context(|| format!("mockup {}", path.display()))?.to_rgb8();
                img = side_by_side(&mockup, &img);
            }
            img.save(&out).with_context(|| format!("writing {}", out.display()))?;
            println!("{} {}x{} px, {cols}x{rows} cells of {}x{} px", out.display(), img.width(), img.height(), raster.cell_w, raster.cell_h);
            Ok(())
        }
    }
}

/// The live terminal UI on a fixture state. Same `AppState` and `draw` as `screen`; keys and
/// clicks go through `frontend_tui::input`, and the next screen's data comes from fixtures until
/// the core exists.
fn preview(name: &str, theme: &str) -> Result<()> {
    use std::collections::HashMap;

    use frontend_tui::images::{self, Graphic};
    use frontend_tui::input::{self, Key, Outcome};
    use frontend_tui::model::{ActionKind, Mode};
    use frontend_tui::theme::BUILTIN;
    use frontend_tui::view::Drawn;
    use ratatui::crossterm::event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton,
        MouseEventKind,
    };
    use ratatui::crossterm::execute;
    use ratatui::layout::Size;
    use ratatui_image::picker::{Picker, ProtocolType};
    use ratatui_image::protocol::Protocol;
    use ratatui_image::{Image, Resize};

    let fixture = fixtures::by_name(name).ok_or_else(|| anyhow!("unknown screen {name:?} (see `screen --list`)"))?;
    let mut state = (fixture.build)();
    let mut theme_idx = BUILTIN.iter().position(|(id, _)| *id == theme).ok_or_else(|| anyhow!("unknown theme {theme:?}"))?;
    let mut current = Theme::builtin(BUILTIN[theme_idx].0)?;

    let mut terminal = ratatui::init();
    execute!(std::io::stdout(), EnableMouseCapture)?;
    // Which graphics protocol the terminal speaks and its cell size in pixels; asked once, after
    // raw mode is on (ratatui-image 11 examples). Halfblocks means no real graphics: keep the
    // text fallback the view already drew.
    let mode = config::graphics();
    let forced = match mode {
        config::Graphics::Sixel => Some(ProtocolType::Sixel),
        config::Graphics::Kitty => Some(ProtocolType::Kitty),
        config::Graphics::Iterm2 => Some(ProtocolType::Iterm2),
        _ => None,
    };
    let ask = match mode {
        config::Graphics::Off => false,
        config::Graphics::Auto => graphics_terminal(),
        _ => true,
    };
    // The terminal's answer decides whether pictures are possible at all: a terminal that
    // answers no graphics query (Halfblocks) shows the text fallbacks, whatever was forced.
    // Forcing a protocol on such a terminal printed kitty placeholders as boxes in Ptyxis
    // (operator, 2026-11-01). A forced protocol only picks among what the terminal supports.
    let picker = ask
        .then(|| Picker::from_query_stdio().ok())
        .flatten()
        .filter(|p| p.protocol_type() != ProtocolType::Halfblocks)
        .map(|mut p| {
            if let Some(proto) = forced {
                p.set_protocol_type(proto);
            }
            p
        });
    let launched = std::time::Instant::now();
    let mut pictures: HashMap<(Graphic, String, u16, u16), Protocol> = HashMap::new();
    let result = (|| -> Result<()> {
        let mut drawn = Drawn::default();
        loop {
            state.pictures = picker.is_some();
            // Input still queued (a burst of wheel events, fast typing) is handled first and
            // drawn once after, instead of one frame per event.
            if !event::poll(std::time::Duration::ZERO)? {
                terminal.draw(|f| {
                    drawn = frontend_tui::draw(f, &state, &current);
                    let Some(picker) = &picker else { return };
                    for p in &drawn.placements {
                        let key = (p.graphic.clone(), current.id.clone(), p.rect.width, p.rect.height);
                        if !pictures.contains_key(&key) {
                            let fs = picker.font_size();
                            let px = images::rasterize(&p.graphic, &current, (p.rect.width * fs.width) as u32, (p.rect.height * fs.height) as u32);
                            if let Ok(proto) = picker.new_protocol(
                                image::DynamicImage::ImageRgba8(px),
                                Size::new(p.rect.width, p.rect.height),
                                Resize::Fit(None),
                            ) {
                                pictures.insert(key.clone(), proto);
                            }
                        }
                        if let Some(proto) = pictures.get(&key) {
                            f.render_widget(Image::new(proto), p.rect);
                        }
                    }
                })?;
                // The launch blink: redraw when the eyes change, otherwise wait for input.
                let (eyes, next) = images::eyes_at(launched.elapsed().as_millis() as u64);
                if state.eyes != eyes {
                    state.eyes = eyes;
                    continue;
                }
                let wait = next.map(|n| n.saturating_sub(launched.elapsed().as_millis() as u64));
                if let Some(wait) = wait
                    && !event::poll(std::time::Duration::from_millis(wait))?
                {
                    continue;
                }
            }
            let outcome = match event::read()? {
                Event::Key(k) if k.kind == KeyEventKind::Press => {
                    if k.code == KeyCode::Char('c') && k.modifiers.contains(KeyModifiers::CONTROL) {
                        return Ok(());
                    }
                    if k.code == KeyCode::F(2) {
                        theme_idx = (theme_idx + 1) % BUILTIN.len();
                        current = Theme::builtin(BUILTIN[theme_idx].0)?;
                        continue;
                    }
                    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
                    let alt = k.modifiers.contains(KeyModifiers::ALT);
                    let key = match k.code {
                        KeyCode::Char(c) if ctrl => Key::Ctrl(c.to_ascii_lowercase()),
                        KeyCode::Char(c) if alt => Key::Alt(c),
                        KeyCode::PageUp => Key::PageUp,
                        KeyCode::PageDown => Key::PageDown,
                        KeyCode::Home => Key::Home,
                        KeyCode::End => Key::End,
                        KeyCode::Enter => Key::Enter,
                        KeyCode::Esc => Key::Esc,
                        KeyCode::Up => Key::Up,
                        KeyCode::Down => Key::Down,
                        KeyCode::Left => Key::Left,
                        KeyCode::Right => Key::Right,
                        KeyCode::Tab | KeyCode::BackTab => Key::Tab,
                        KeyCode::Backspace => Key::Backspace,
                        KeyCode::Char(c) => Key::Char(c),
                        _ => continue,
                    };
                    input::key(&mut state, key)
                }
                Event::Mouse(m) if m.kind == MouseEventKind::Down(MouseButton::Left) => {
                    input::click(&mut state, &drawn, m.column, m.row)
                }
                Event::Mouse(m) if matches!(m.kind, MouseEventKind::ScrollUp | MouseEventKind::ScrollDown) => {
                    input::scroll(&mut state, if m.kind == MouseEventKind::ScrollUp { 3 } else { -3 });
                    continue;
                }
                _ => continue,
            };
            match outcome {
                Outcome::Quit => return Ok(()),
                Outcome::LeaveSplash => state = fixtures::startup(),
                Outcome::OpenSession(_) => state = fixtures::main_session(),
                Outcome::Action(i) => {
                    let kind = match &state.mode {
                        Mode::Startup(v) => v.actions.get(i).map(|a| a.kind),
                        _ => None,
                    };
                    match kind {
                        Some(ActionKind::NewSession) => state = fixtures::welcome(),
                        Some(ActionKind::Resume) => state = fixtures::main_session(),
                        Some(ActionKind::Settings) => {
                            state = fixtures::welcome();
                            if let Mode::Session(w) = &mut state.mode {
                                w.overlay = Some(frontend_tui::model::Overlay::Settings(frontend_tui::overlay::sample_settings()));
                            }
                        }
                        // Settings, tour and help screens come in later stages.
                        _ => {}
                    }
                }
                Outcome::Stay | Outcome::OpenSettings => {}
            }
        }
    })();
    let _ = execute!(std::io::stdout(), DisableMouseCapture);
    ratatui::restore();
    result
}

/// In `auto` mode, whether to ask the terminal about graphics at all. ratatui-image 11's query
/// leaves a thread reading stdin after its timeout (it eats the next key and later turns raw
/// mode off), so only terminals whose environment says they have graphics are asked; the config
/// (`multitool setup --graphics`) or MULTITOOL_GRAPHICS decide for the rest.
fn graphics_terminal() -> bool {
    let env = |k: &str| std::env::var(k).unwrap_or_default();
    let term = env("TERM");
    let program = env("TERM_PROGRAM");
    !env("KITTY_WINDOW_ID").is_empty()
        || !env("WEZTERM_EXECUTABLE").is_empty()
        || !env("KONSOLE_VERSION").is_empty()
        || !env("WT_SESSION").is_empty()
        || term.contains("kitty")
        || term.starts_with("foot")
        || term.contains("ghostty")
        // GNOME Terminal and other VTE terminals answer the query (sixel when enabled).
        || !env("VTE_VERSION").is_empty()
        || ["WezTerm", "ghostty", "iTerm.app", "rio"].contains(&program.as_str())
}
