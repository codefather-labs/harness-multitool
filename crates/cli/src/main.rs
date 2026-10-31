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
        #[arg(long, default_value = "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf")]
        font: PathBuf,
        #[arg(long, default_value = "/usr/share/fonts/truetype/dejavu/DejaVuSansMono-Bold.ttf")]
        font_bold: PathBuf,
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

            let raster = Raster::new(&font, &font_bold, px, cell_h)?;
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

/// The live terminal UI on a fixture state. Same `AppState` and `draw` as `screen`.
fn preview(name: &str, theme: &str) -> Result<()> {
    use frontend_tui::model::Mode;
    use frontend_tui::theme::BUILTIN;
    use std::collections::HashMap;

    use frontend_tui::images::{self, Graphic};
    use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
    use ratatui::layout::Size;
    use ratatui_image::picker::{Picker, ProtocolType};
    use ratatui_image::protocol::Protocol;
    use ratatui_image::{Image, Resize};

    let fixture = fixtures::by_name(name).ok_or_else(|| anyhow!("unknown screen {name:?} (see `screen --list`)"))?;
    let mut state = (fixture.build)();
    let mut theme_idx = BUILTIN.iter().position(|(id, _)| *id == theme).ok_or_else(|| anyhow!("unknown theme {theme:?}"))?;
    let mut current = Theme::builtin(BUILTIN[theme_idx].0)?;

    let mut terminal = ratatui::init();
    // Which graphics protocol the terminal speaks and its cell size in pixels; asked once, after
    // raw mode is on (ratatui-image 11 examples). Halfblocks means no real graphics: keep the
    // text fallback the view already drew.
    let picker = Picker::from_query_stdio().ok().filter(|p| p.protocol_type() != ProtocolType::Halfblocks);
    let mut pictures: HashMap<(Graphic, String, u16, u16), Protocol> = HashMap::new();
    let result = (|| -> Result<()> {
        loop {
            terminal.draw(|f| {
                let placements = frontend_tui::draw(f, &state, &current);
                let Some(picker) = &picker else { return };
                for p in placements {
                    let key = (p.graphic, current.id.clone(), p.rect.width, p.rect.height);
                    if !pictures.contains_key(&key) {
                        let fs = picker.font_size();
                        let px = images::rasterize(p.graphic, &current, (p.rect.width * fs.width) as u32, (p.rect.height * fs.height) as u32);
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
            let Event::Key(k) = event::read()? else { continue };
            if k.kind != KeyEventKind::Press {
                continue;
            }
            let Mode::Session(v) = &mut state.mode;
            match k.code {
                KeyCode::Esc => return Ok(()),
                KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => return Ok(()),
                KeyCode::F(2) => {
                    theme_idx = (theme_idx + 1) % BUILTIN.len();
                    current = Theme::builtin(BUILTIN[theme_idx].0)?;
                }
                KeyCode::Enter => v.composer.text.clear(),
                KeyCode::Backspace => {
                    v.composer.text.pop();
                }
                KeyCode::Char(c) => v.composer.text.push(c),
                _ => {}
            }
        }
    })();
    ratatui::restore();
    result
}
