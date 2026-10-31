//! `multitool`: the harness binary. For now it carries the design-debug `screen` command, which
//! renders a named state of the terminal UI to PNG (journal: tui-screens specification).

use std::path::PathBuf;

use anyhow::{Context, Result, anyhow, bail};
use clap::{Parser, Subcommand};
use frontend_tui::{Theme, fixtures};
use tui_snapshot::{Raster, side_by_side};

#[derive(Parser)]
#[command(name = "multitool", version, about = "harness-multitool")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Open the terminal UI on a named state (default: welcome). F2 cycles themes, Esc quits.
    Preview {
        #[arg(default_value = "welcome")]
        name: String,
        #[arg(long, default_value = "dark")]
        theme: String,
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
        Command::Logo { theme, width, png } => {
            let theme = Theme::builtin(&theme)?;
            let height = width * 82 / 530;
            let pic = frontend_tui::images::rasterize(&frontend_tui::images::Graphic::Logo, &theme, width, height);
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
        Command::Screen { name, list, theme, size, png, compare, mockups, font, font_bold, px, cell_h } => {
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
            let mut img = raster.render(&(fixture.build)(), &theme, cols, rows);
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
    let picker = graphics_terminal()
        .then(|| Picker::from_query_stdio().ok())
        .flatten()
        .filter(|p| p.protocol_type() != ProtocolType::Halfblocks);
    let mut pictures: HashMap<(Graphic, String, u16, u16), Protocol> = HashMap::new();
    let result = (|| -> Result<()> {
        loop {
            let mut drawn = Drawn::default();
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
                    let key = match k.code {
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
                        // Settings, tour and help screens come in later stages.
                        _ => {}
                    }
                }
                Outcome::Stay | Outcome::OpenSettings | Outcome::Send => {}
            }
        }
    })();
    let _ = execute!(std::io::stdout(), DisableMouseCapture);
    ratatui::restore();
    result
}

/// Whether to ask the terminal about graphics at all. ratatui-image 11's query leaves a thread
/// reading stdin after its timeout (it eats the next key and later turns raw mode off), so only
/// terminals that say they have graphics are asked. MULTITOOL_GRAPHICS=1 or 0 overrides.
fn graphics_terminal() -> bool {
    let env = |k: &str| std::env::var(k).unwrap_or_default();
    match env("MULTITOOL_GRAPHICS").as_str() {
        "1" => return true,
        "0" => return false,
        _ => {}
    }
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
