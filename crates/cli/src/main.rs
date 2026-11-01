//! `multitool`: the harness binary. The terminal UI runs real sessions (harness-session); the
//! debug commands `screen` and `trace` show a named UI state or a loop scenario on their own.

use std::path::PathBuf;

use anyhow::{Context, Result, anyhow, bail};
use clap::{Parser, Subcommand};
use frontend_tui::{Theme, fixtures};
use tui_snapshot::{Raster, side_by_side};

mod config;
mod plugins;
mod reload;

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
        /// Debug: new sessions run a named scenario on test doubles instead of a real model
        /// (see `multitool trace`).
        #[arg(long)]
        script: Option<String>,
        /// Continue from a hand-off file: the hot reload's new process starts this way.
        #[arg(long)]
        handoff: Option<PathBuf>,
    },
    /// Debug the agent loop: run a named scenario on test doubles and print its log, one line per event.
    Trace {
        /// Scenario name; omit to list them.
        name: Option<String>,
        /// Print the log as JSON lines, as the session store keeps it.
        #[arg(long)]
        json: bool,
    },
    /// The daemon: the single entry point of callbacks (the terminal UI starts it when needed).
    Daemon {
        /// The port on localhost; by default one is picked.
        #[arg(long, default_value_t = 0)]
        port: u16,
        /// Don't start one: print what the running daemon counts.
        #[arg(long)]
        status: bool,
    },
    /// Debug: one message through a real session with no UI; prints its log, one line per event.
    Run {
        /// The message for the agent.
        prompt: String,
        /// Answer every permission prompt with yes; otherwise no.
        #[arg(long)]
        yes: bool,
        /// Make it a task: done only when this command passes, and the project's acceptance
        /// checks. Repeatable.
        #[arg(long)]
        verify: Vec<String>,
        /// Make it a task with only the acceptance checks.
        #[arg(long)]
        task: bool,
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
        /// Fixture name; omit with --list or --scenario.
        name: Option<String>,
        /// Debug: instead of a fixture, draw the session a scenario of `multitool trace` leaves.
        #[arg(long)]
        scenario: Option<String>,
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
    let command = Cli::parse().command.unwrap_or(Command::Preview { name: "welcome".into(), theme: "dark".into(), script: None, handoff: None });
    match command {
        Command::Preview { name, theme, script, handoff } => preview(&name, &theme, script.as_deref(), handoff.as_deref()),
        Command::Trace { name, json } => trace(name.as_deref(), json),
        Command::Run { prompt, yes, verify, task } => run_once(&prompt, yes, (task || !verify.is_empty()).then_some(verify)),
        Command::Daemon { status: true, .. } => {
            let client = harness_daemon::client::Client::find(&config::data_dir()?).ok_or_else(|| anyhow!("no daemon is running"))?;
            println!("{}", client.stats().map_err(|e| anyhow!(e))?);
            Ok(())
        }
        Command::Daemon { port, .. } => {
            let dir = config::data_dir()?;
            // The port it had before if it is free: callback URLs stay as they were.
            let wanted = if port == 0 { harness_daemon::last_port(&dir).unwrap_or(0) } else { port };
            let daemon = harness_daemon::Daemon::start_with(&dir, wanted, plugins::callback_handlers())
                .or_else(|_| harness_daemon::Daemon::start_with(&dir, port, plugins::callback_handlers()))
                .map_err(|e| anyhow!(e))?;
            println!("daemon on 127.0.0.1:{}", daemon.port());
            daemon.wait_for_shutdown();
            Ok(())
        }
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
        Command::Screen { name, scenario, list, theme, size, png, compare, no_graphics, eyes_closed, mockups, font, font_bold, px, cell_h } => {
            if list {
                for f in fixtures::ALL {
                    println!("{}\t{}", f.name, f.mockup);
                }
                return Ok(());
            }
            let scenario_state = scenario.as_deref().map(scenario_session).transpose()?;
            let name = if scenario.is_some() { "welcome".to_string() } else { name.ok_or_else(|| anyhow!("a screen name is required (see --list)"))? };
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
            let mut state = scenario_state.unwrap_or_else(|| (fixture.build)());
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

/// A log time as a card's stamp.
fn local_time(at_ms: u64) -> String {
    use chrono::TimeZone;
    chrono::Local.timestamp_millis_opt(at_ms as i64).single().map_or_else(String::new, |t| t.format("%H:%M").to_string())
}

/// The line that tells a tab's agent where callbacks reach it.
fn callback_note(urls: &std::sync::Mutex<std::collections::HashMap<u64, String>>, tab: u64) -> Option<String> {
    urls.lock().ok()?.get(&tab).map(|url| {
        format!("Callbacks from outside reach this session at {url}: POST JSON with a unique uuid, a type (notification, journal, flowstate_update, or a type of this project) and its fields.")
    })
}

/// What a session gets from this binary.
fn host_for(note: std::sync::Arc<dyn Fn(u64) -> Option<String> + Send + Sync>, reload: std::sync::Arc<std::sync::atomic::AtomicBool>, summary: serde_json::Value) -> harness_session::Host {
    let plugins = std::sync::Arc::new(move || {
        let mut all = summary.clone();
        let handlers = config::data_dir().ok().and_then(|d| harness_daemon::client::Client::find(&d)).and_then(|c| c.handlers().ok()).unwrap_or_default();
        all["callback_handlers_custom"] = serde_json::json!(handlers.iter().map(|(name, types)| serde_json::json!({"name": name, "types": types})).collect::<Vec<_>>());
        all["callback_handlers_standard"] = serde_json::json!(["notification", "journal", "flowstate_update"]);
        all
    });
    harness_session::Host { note, plugins, reload }
}

fn selection_summary(s: &harness_session::Settings) -> serde_json::Value {
    serde_json::json!({"provider": s.provider, "model": s.model, "tools": s.tools, "hooks": s.hooks.iter().map(|h| h.name.clone().unwrap_or_else(|| h.command.clone())).collect::<Vec<_>>()})
}

/// A card that belongs to no turn of the agent's goes straight into the transcript.
fn push_card(view: &mut frontend_tui::model::SessionView, card: frontend_tui::model::Item, _now: &str) {
    view.transcript.push(card);
    view.scroll = 0;
}

/// How long ago, as the startup page says it.
fn age(ms: u64) -> String {
    match ms / 60_000 {
        0 => "just now".to_string(),
        m @ 1..=59 => format!("{m} min ago"),
        m @ 60..=1439 => format!("{} h ago", m / 60),
        1440..=2879 => "1 day ago".to_string(),
        m => format!("{} days ago", m / 1440),
    }
}

/// The working directory as the header shows it.
fn shown_cwd() -> String {
    let cwd = std::env::current_dir().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    match dirs::home_dir().and_then(|h| cwd.strip_prefix(&*h.to_string_lossy()).map(str::to_string)) {
        Some(rest) => format!("~{rest}"),
        None => cwd,
    }
}

/// The current git branch, when this is a repository root on a branch.
fn git_branch() -> Option<String> {
    let head = std::fs::read_to_string(".git/HEAD").ok()?;
    head.trim().strip_prefix("ref: refs/heads/").map(str::to_string)
}

/// A fresh session view on the given model: the status line says what will really run.
fn live_session(provider: &str, model: &str, budget: &str) -> frontend_tui::AppState {
    let mut state = fixtures::welcome();
    state.header.cwd = shown_cwd();
    state.header.branch = git_branch();
    if let frontend_tui::model::Mode::Session(w) = &mut state.mode {
        w.live = true;
        w.view_mut().status.fields = [("ADAPTER", provider), ("MODEL", model), ("TOKENS", "0"), ("COST", "$0.00"), ("BUDGET", budget)]
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .to_vec();
    }
    state
}

/// The screen a scenario of `multitool trace` leaves: the debug view of the UI's wiring.
fn scenario_session(name: &str) -> Result<frontend_tui::AppState> {
    let setup = harness_scenarios::setup(name).ok_or_else(|| anyhow!("unknown scenario {name:?} (run `multitool trace` for the list)"))?;
    let t = harness_scenarios::run(setup);
    let mut state = live_session("scripted", "scripted-1", "none");
    if let frontend_tui::model::Mode::Session(w) = &mut state.mode {
        let mut feed = frontend_tui::live::Feed::new(local_time, 200_000);
        for entry in &t.stored {
            feed.apply(w.view_mut(), entry);
        }
        feed.run_ended(w.view_mut());
    }
    Ok(state)
}

/// An event from the daemon for one tab.
type TabEvent = (u64, harness_daemon::WindowEvent);

/// The sessions behind the tabs, and what ties them to the view.
struct Live {
    sessions: harness_session::Sessions,
    feeds: std::collections::HashMap<u64, frontend_tui::live::Feed>,
    /// Permission prompts waiting for the one on screen to be answered.
    asks: std::collections::VecDeque<frontend_tui::model::PermissionView>,
    context_window: u64,
    root: String,
    /// On real plugins, not a debug script: only then are settings and stored sessions real.
    real: bool,
    /// This folder's stored sessions, as the startup page lists them.
    recent: Vec<harness_session::SessionInfo>,
    provider: String,
    model: String,
    budget: String,
    /// Each tab's callback URL, for its agent's system prompt.
    urls: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<u64, String>>>,
    /// Tabs already registered with the daemon.
    windows: std::collections::HashSet<u64>,
    /// Each tab's window at the daemon, for the hand-off.
    registered: std::collections::HashMap<u64, harness_daemon::client::Registered>,
    /// The agent's `plugins_reload` tool raises this.
    reload_asked: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// A rebuild in progress.
    reloading: Option<std::sync::mpsc::Receiver<Result<(), String>>>,
    /// Built; the swap waits for a safe point.
    ready_to_swap: bool,
    window_events: (std::sync::mpsc::Sender<TabEvent>, std::sync::mpsc::Receiver<TabEvent>),
}

impl Live {
    /// Gives each new tab of a real session its window at the daemon, and listens to it.
    fn connect_windows(&mut self, state: &mut frontend_tui::AppState) {
        let frontend_tui::model::Mode::Session(w) = &mut state.mode else { return };
        if !w.live || !self.real {
            return;
        }
        let tabs: Vec<u64> = w.tabs.iter().map(|t| t.id).collect();
        let before = self.windows.len();
        for tab in tabs {
            if !self.windows.insert(tab) {
                continue;
            }
            let Ok(dir) = config::data_dir() else { continue };
            let Ok(program) = std::env::current_exe() else { continue };
            let Ok(client) = harness_daemon::client::Client::find_or_start(&dir, &program) else { continue };
            let Ok(window) = client.register(&self.root) else { continue };
            self.listen(tab, client, window);
        }
        // The daemon may have just started: the settings should say so.
        if self.windows.len() != before {
            w.callbacks = Some(self.callbacks_page());
        }
    }

    /// Remembers a tab's window and hears its events from now on.
    fn listen(&mut self, tab: u64, client: harness_daemon::client::Client, window: harness_daemon::client::Registered) {
        if let Ok(mut urls) = self.urls.lock() {
            urls.insert(tab, window.url.clone());
        }
        self.registered.insert(tab, window.clone());
        let sender = self.window_events.0.clone();
        std::thread::spawn(move || {
            let mut after = 0;
            loop {
                match client.poll(&window.window, after) {
                    Ok(events) => {
                        for e in events {
                            after = after.max(e.seq);
                            if sender.send((tab, e.event)).is_err() {
                                return;
                            }
                        }
                    }
                    Err(_) => std::thread::sleep(std::time::Duration::from_secs(1)),
                }
            }
        });
    }

    /// The new process after a hot reload: every tab back from its session's log.
    fn restore(&mut self, handoff: &reload::Handoff) -> frontend_tui::AppState {
        use frontend_tui::model::{Mode, Tab, TabIcon, TabMark};
        let mut state = self.fresh_session();
        let data = config::data_dir().map(|d| d.to_string_lossy().into_owned()).unwrap_or_default();
        let client = match (config::data_dir(), std::env::current_exe()) {
            (Ok(dir), Ok(program)) => harness_daemon::client::Client::find_or_start(&dir, &program).ok(),
            _ => None,
        };
        let Mode::Session(w) = &mut state.mode else { return state };
        let template = w.view().clone();
        w.tabs.clear();
        for saved in &handoff.tabs {
            let mut view = template.clone();
            if saved.session.is_some() {
                view.transcript.clear();
            }
            view.composer.text = saved.draft.clone();
            view.scroll = saved.scroll;
            let tab = Tab::new(&saved.title, TabIcon::Prompt, TabMark::None, view);
            let id = tab.id;
            w.tabs.push(tab);
            let view = &mut w.tabs.last_mut().expect("just pushed").view;
            if let Some(session) = &saved.session {
                let mut feed = frontend_tui::live::Feed::new(local_time, self.context_window);
                for entry in harness_session::read_log(&data, &self.root, session) {
                    feed.apply(view, &entry);
                    w.journal.extend(feed.take_known());
                }
                feed.run_ended(view);
                self.feeds.insert(id, feed);
            }
            if let (Some(client), Some(window), Some(url)) = (&client, &saved.window, &saved.url) {
                self.windows.insert(id);
                self.listen(id, client.clone(), harness_daemon::client::Registered { window: window.clone(), url: url.clone() });
            }
            // After the URL is known: the session's agent is told it.
            if let Some(session) = &saved.session {
                self.sessions.resume(id, session);
            }
        }
        w.active = handoff.active.min(w.tabs.len().saturating_sub(1));
        w.callbacks = Some(self.callbacks_page());
        state
    }

    /// Moves a hot reload along: the build's result, then the swap at a safe point.
    fn tick_reload(&mut self, state: &mut frontend_tui::AppState, theme: &str) -> Result<bool> {
        use frontend_tui::model::{Item, Mode, Overlay};
        let Mode::Session(w) = &mut state.mode else { return Ok(false) };
        let note = |w: &mut frontend_tui::model::Workspace, text: String| {
            w.view_mut().transcript.push(Item::Tips { label: "RELOAD".into(), lines: text.lines().map(str::to_string).collect() });
        };
        let mut changed = false;
        if self.reload_asked.swap(false, std::sync::atomic::Ordering::SeqCst) && self.reloading.is_none() && !self.ready_to_swap {
            self.reloading = Some(reload::start_build());
            note(w, "Rebuilding…".to_string());
            changed = true;
        }
        if let Some(result) = self.reloading.as_ref().and_then(|r| r.try_recv().ok()) {
            self.reloading = None;
            changed = true;
            match result {
                Ok(()) => self.ready_to_swap = true,
                Err(message) => note(w, format!("Nothing was swapped: {message}")),
            }
        }
        // CON-50: never during a model turn or a tool call, never over a permission prompt.
        let busy = w.tabs.iter().any(|t| t.view.generating) || matches!(w.overlay, Some(Overlay::Permission(_))) || !self.asks.is_empty();
        if !self.ready_to_swap || busy {
            return Ok(changed);
        }
        let handoff = reload::Handoff {
            active: w.active,
            theme: theme.to_string(),
            tabs: w
                .tabs
                .iter()
                .map(|t| reload::TabHandoff {
                    title: t.title.clone(),
                    session: self.sessions.session_of(t.id).map(str::to_string),
                    window: self.registered.get(&t.id).map(|r| r.window.clone()),
                    url: self.registered.get(&t.id).map(|r| r.url.clone()),
                    scroll: t.view.scroll,
                    draft: t.view.composer.text.clone(),
                })
                .collect(),
        };
        let dir = config::data_dir()?;
        let path = reload::write_handoff(&dir, &handoff)?;
        // The daemon holds the custom handlers, so it is replaced too; the new process starts it.
        if let Some(client) = harness_daemon::client::Client::find(&dir) {
            let _ = client.shutdown();
        }
        reload::swap(&path, theme)?;
        Ok(true)
    }

    /// What the daemon said reaches the view, or the agent.
    fn receive_daemon(&mut self, state: &mut frontend_tui::AppState) -> bool {
        use frontend_tui::model::{Item, Mode};
        use harness_daemon::WindowEvent;
        let events: Vec<_> = self.window_events.1.try_iter().collect();
        let any = !events.is_empty();
        let Mode::Session(w) = &mut state.mode else { return any };
        let now = local_time(chrono::Utc::now().timestamp_millis() as u64);
        let window_size = self.context_window;
        for (tab, event) in events {
            let Some(index) = w.tabs.iter().position(|t| t.id == tab) else { continue };
            match event {
                WindowEvent::Notification { label, text, .. } => {
                    w.tabs[index].view.generating = true;
                    self.sessions.inbound(tab, label, text);
                }
                WindowEvent::Journal { record } => {
                    let card = frontend_tui::live::journal_card(&record, &now);
                    w.journal.push(card.clone());
                    push_card(&mut w.tabs[index].view, Item::Journal(card), &now);
                }
                WindowEvent::Flow { map, instance, node, state, off_chart, path } => {
                    let feed = self.feeds.entry(tab).or_insert_with(|| frontend_tui::live::Feed::new(local_time, window_size));
                    feed.flow(&mut w.tabs[index].view, &map, &instance, node, &state, off_chart, &path, &now, chrono::Utc::now().timestamp_millis() as u64);
                }
            }
        }
        any
    }

    fn new(script: Option<&str>) -> Result<Live> {
        let root = std::env::current_dir()?.to_string_lossy().into_owned();
        let settings = config::load()?.session;
        let budget = settings.max_cost_usd.map_or("none".to_string(), |d| format!("${d:.2}"));
        let urls: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<u64, String>>> = Default::default();
        let reload_asked: std::sync::Arc<std::sync::atomic::AtomicBool> = Default::default();
        let (factory, provider, model, context_window): (harness_session::Factory, String, String, u64) = match script {
            Some(name) => {
                harness_scenarios::setup(name).ok_or_else(|| anyhow!("unknown scenario {name:?} (run `multitool trace` for the list)"))?;
                let name = name.to_string();
                let factory: harness_session::Factory = std::sync::Arc::new(move |_| Ok(harness_scenarios::parts(harness_scenarios::setup(&name).expect("checked above"))));
                (factory, "scripted".into(), "scripted-1".into(), 200_000)
            }
            None => {
                let (provider, model, window) = (settings.provider.clone(), settings.model.clone(), settings.context_window());
                // A tab's agent is told its callback URL once the daemon gave it one.
                let urls = urls.clone();
                let note = std::sync::Arc::new(move |tab: u64| callback_note(&urls, tab));
                let host = host_for(note, reload_asked.clone(), selection_summary(&settings));
                (harness_session::with_host(settings, root.clone(), config::data_dir()?, host), provider, model, window)
            }
        };
        Ok(Live { sessions: harness_session::Sessions::new(factory, root.clone()), feeds: Default::default(), asks: Default::default(), context_window, root, real: script.is_none(), recent: Vec::new(), provider, model, budget, urls, windows: Default::default(), window_events: std::sync::mpsc::channel(), registered: Default::default(), reload_asked, reloading: None, ready_to_swap: false })
    }

    fn fresh_session(&self) -> frontend_tui::AppState {
        let mut state = live_session(&self.provider, &self.model, &self.budget);
        if let frontend_tui::model::Mode::Session(w) = &mut state.mode {
            w.callbacks = Some(self.callbacks_page());
        }
        state
    }

    /// What the settings' Callbacks tab shows: this project's files and this build's handlers.
    fn callbacks_page(&self) -> frontend_tui::model::CallbacksPage {
        use frontend_tui::model::{CallbacksPage, ConditionRow, CustomRow};
        let project = harness_daemon::conditions::read_project(&self.root).unwrap_or_default();
        let client = config::data_dir().ok().and_then(|d| harness_daemon::client::Client::find(&d));
        let handlers = client.as_ref().and_then(|c| c.handlers().ok()).unwrap_or_default();
        let mut standard = frontend_tui::overlay::standard_rows();
        for row in &mut standard {
            // Any handler of this build can stand in for a standard type.
            for (name, _) in &handlers {
                row.implementations.insert(row.implementations.len() - 1, name.clone());
            }
            if let Some(chosen) = project.handlers.get(&row.name).and_then(|h| row.implementations.iter().position(|i| i == h)) {
                row.chosen = chosen;
            }
        }
        let conditions = harness_daemon::conditions::read_configs(&self.root)
            .unwrap_or_default()
            .into_iter()
            .map(|c| {
                let window = c.within_s.map_or(String::new(), |s| format!(" in {s} s"));
                let rule = format!("{} signal(s){window}, {} s quiet", c.at_least, c.cooldown_s);
                ConditionRow { on: !project.disabled.contains(&c.name), name: c.name, listens_to: c.on, rule, policy: c.policies.join(", ") }
            })
            .collect();
        let custom = handlers
            .iter()
            .filter(|(_, types)| !types.is_empty())
            .map(|(name, types)| CustomRow { name: name.clone(), implementation: types.join(", "), on: !project.disabled.contains(name) })
            .collect();
        let daemon = client.map_or("not running".to_string(), |c| format!("running on 127.0.0.1:{}", c.port()));
        CallbacksPage { standard, custom, conditions, daemon, cursor: 0, open: None }
    }

    fn reload_daemon(&self) -> String {
        let reply = config::data_dir().ok().and_then(|d| harness_daemon::client::Client::find(&d)).map(|c| c.reload());
        match reply {
            Some(Ok(_)) => "The daemon read the project's callback files again.".to_string(),
            Some(Err(e)) => format!("The daemon did not reload: {e}"),
            None => "The daemon is not running; it will read the files when it starts.".to_string(),
        }
    }

    /// Saves the choice and makes it the base of new sessions. A running session keeps its model.
    fn save_settings(&mut self, provider: &str, model: &str) -> Result<()> {
        let mut c = config::load()?;
        c.session.provider = provider.to_string();
        c.session.model = model.to_string();
        config::save(&c)?;
        if self.real {
            self.context_window = c.session.context_window();
            let note = {
                let urls = self.urls.clone();
                std::sync::Arc::new(move |tab: u64| callback_note(&urls, tab))
            };
            let host = host_for(note, self.reload_asked.clone(), selection_summary(&c.session));
            self.sessions.set_factory(harness_session::with_host(c.session, self.root.clone(), config::data_dir()?, host));
            self.provider = provider.to_string();
            self.model = model.to_string();
        }
        Ok(())
    }

    /// The startup page, listing this folder's real sessions.
    fn startup(&mut self) -> frontend_tui::AppState {
        let mut state = fixtures::startup();
        state.header.cwd = shown_cwd();
        state.header.branch = git_branch();
        self.recent = config::data_dir().map(|d| harness_session::recent(&d.to_string_lossy(), &self.root)).unwrap_or_default();
        if !self.real {
            self.recent.clear();
        }
        if let frontend_tui::model::Mode::Startup(v) = &mut state.mode {
            let now = chrono::Utc::now().timestamp_millis() as u64;
            v.sessions = self
                .recent
                .iter()
                .map(|s| frontend_tui::model::RecentSession {
                    title: s.title.clone(),
                    path: shown_cwd(),
                    branch: git_branch().unwrap_or_default(),
                    age: age(now.saturating_sub(s.last_ms)),
                    model: s.model.clone(),
                })
                .collect();
            v.selected_session = 0;
        }
        state
    }

    /// Opens stored session `i`: its log is drawn, and the next message continues it.
    fn open_recent(&mut self, i: usize) -> Option<frontend_tui::AppState> {
        let info = self.recent.get(i)?.clone();
        let data = config::data_dir().ok()?;
        let log = harness_session::read_log(&data.to_string_lossy(), &self.root, &info.id);
        let mut state = self.fresh_session();
        if let frontend_tui::model::Mode::Session(w) = &mut state.mode {
            let tab = w.tabs[w.active].id;
            let mut feed = frontend_tui::live::Feed::new(local_time, self.context_window);
            let view = w.view_mut();
            view.transcript.clear();
            for entry in &log {
                feed.apply(view, entry);
            }
            feed.run_ended(view);
            self.feeds.insert(tab, feed);
            self.sessions.resume(tab, &info.id);
        }
        Some(state)
    }

    /// What the user did goes to the sessions; returns whether anything changed on screen.
    fn send(&mut self, state: &mut frontend_tui::AppState) {
        use frontend_tui::model::{Mode, Outgoing, Overlay};
        let Mode::Session(w) = &mut state.mode else { return };
        for out in std::mem::take(&mut w.outbox) {
            match out {
                Outgoing::Prompt { tab, text } => {
                    if let Some(t) = w.tabs.iter_mut().find(|t| t.id == tab) {
                        t.view.generating = true;
                    }
                    self.sessions.prompt(tab, text);
                }
                Outgoing::Task { tab, text, steps } => {
                    let refused = self.sessions.task(tab, text, steps).err();
                    if let Some(t) = w.tabs.iter_mut().find(|t| t.id == tab) {
                        match refused {
                            None => t.view.generating = true,
                            Some(reason) => t.view.transcript.push(frontend_tui::model::Item::Tips { label: "TASK REFUSED".into(), lines: vec![reason] }),
                        }
                    }
                }
                Outgoing::Answer { tab, choice } => {
                    let answer = match choice {
                        0 => harness_api::Answer::AllowOnce,
                        1 => harness_api::Answer::AlwaysAllow,
                        _ => harness_api::Answer::Deny,
                    };
                    self.sessions.answer(tab, answer);
                    if w.overlay.is_none() {
                        w.overlay = self.asks.pop_front().map(Overlay::Permission);
                    }
                }
                Outgoing::Stop { tab } => self.sessions.stop(tab),
                Outgoing::SaveCallbacks { handlers, disabled } => {
                    let settings = harness_daemon::conditions::ProjectCallbacks { handlers: handlers.into_iter().collect(), disabled };
                    let note = match harness_daemon::conditions::write_project(&self.root, &settings) {
                        Ok(()) => format!("Saved .harness/callbacks.toml. {}", self.reload_daemon()),
                        Err(e) => format!("Could not save: {e}"),
                    };
                    w.callbacks = Some(self.callbacks_page());
                    w.view_mut().transcript.push(frontend_tui::model::Item::Tips { label: "CALLBACKS".into(), lines: vec![note] });
                }
                Outgoing::Reload => {
                    self.reload_asked.store(true, std::sync::atomic::Ordering::SeqCst);
                }
                Outgoing::ReloadCallbacks => {
                    let note = self.reload_daemon();
                    w.view_mut().transcript.push(frontend_tui::model::Item::Tips { label: "CALLBACKS".into(), lines: vec![note] });
                }
                Outgoing::SaveSettings { provider, model } => {
                    let provider = match provider.as_str() {
                        "Anthropic API" => "anthropic".to_string(),
                        "OpenAI-compatible" => "openai".to_string(),
                        "Local" => "local".to_string(),
                        other => other.to_string(),
                    };
                    let note = match self.save_settings(&provider, &model) {
                        Ok(()) => format!("New sessions use {provider} / {model}."),
                        Err(e) => format!("Could not save the settings: {e}"),
                    };
                    w.view_mut().transcript.push(frontend_tui::model::Item::Tips { label: "SETTINGS".into(), lines: vec![note] });
                }
            }
        }
    }

    /// What the sessions said reaches the view. Returns whether anything did.
    fn receive(&mut self, state: &mut frontend_tui::AppState) -> bool {
        use frontend_tui::live::{Feed, permission};
        use frontend_tui::model::{Item, Mode, Overlay, TabMark};
        use harness_session::FromSession;
        let messages = self.sessions.drain();
        let any = !messages.is_empty();
        let Mode::Session(w) = &mut state.mode else { return any };
        let active = w.tabs.get(w.active).map(|t| t.id);
        for message in messages {
            let window = self.context_window;
            match message {
                FromSession::Entry { tab, entry } => {
                    let feed = self.feeds.entry(tab).or_insert_with(|| Feed::new(local_time, window));
                    if let Some(t) = w.tabs.iter_mut().find(|t| t.id == tab) {
                        feed.apply(&mut t.view, &entry);
                        w.journal.extend(feed.take_known());
                        if Some(tab) != active && matches!(entry.event, harness_api::Event::TurnFinished { .. }) {
                            t.mark = match t.mark {
                                TabMark::Unread(n) => TabMark::Unread(n + 1),
                                _ => TabMark::Unread(1),
                            };
                        }
                    }
                }
                FromSession::Ask { tab, call, capabilities } => {
                    let view = permission(tab, &call, &capabilities, &self.root);
                    if w.overlay.is_none() {
                        w.overlay = Some(Overlay::Permission(view));
                    } else {
                        self.asks.push_back(view);
                    }
                }
                FromSession::Ended { tab, .. } => {
                    let feed = self.feeds.entry(tab).or_insert_with(|| Feed::new(local_time, window));
                    if let Some(t) = w.tabs.iter_mut().find(|t| t.id == tab) {
                        feed.run_ended(&mut t.view);
                    }
                }
                FromSession::Unavailable { tab, message } => {
                    if let Some(t) = w.tabs.iter_mut().find(|t| t.id == tab) {
                        t.view.generating = false;
                        t.view.transcript.push(Item::Tips { label: "CANNOT START THE SESSION".into(), lines: message.lines().map(str::to_string).collect() });
                    }
                }
            }
        }
        any
    }
}

/// `multitool run`: a real session without the UI, for debugging a provider or a tool.
fn run_once(prompt: &str, yes: bool, task: Option<Vec<String>>) -> Result<()> {
    use harness_session::{FromSession, Sessions, real_factory};
    let root = std::env::current_dir()?.to_string_lossy().into_owned();
    let mut sessions = Sessions::new(real_factory(config::load()?.session, root.clone(), config::data_dir()?), root);
    match task {
        Some(steps) => sessions.task(1, prompt.to_string(), steps).map_err(|e| anyhow!(e))?,
        None => sessions.prompt(1, prompt.to_string()),
    }
    loop {
        let Some(message) = sessions.next(std::time::Duration::from_secs(3600)) else { bail!("no event for an hour") };
        match message {
            FromSession::Entry { entry, .. } => println!("{}", harness_scenarios::line(&entry)),
            FromSession::Ask { tab, call, .. } => {
                println!("ask         {} {} -> {}", call.name, call.input, if yes { "yes" } else { "no" });
                sessions.answer(tab, if yes { harness_api::Answer::AllowOnce } else { harness_api::Answer::Deny });
            }
            FromSession::Ended { end, .. } => {
                println!("ended: {end:?}");
                return Ok(());
            }
            FromSession::Unavailable { message, .. } => bail!("{message}"),
        }
    }
}

/// `multitool trace`: the debug view of the loop.
fn trace(name: Option<&str>, json: bool) -> Result<()> {
    use harness_scenarios::{NAMES, line, run, setup};
    let Some(name) = name else {
        NAMES.iter().for_each(|n| println!("{n}"));
        return Ok(());
    };
    let setup = setup(name).ok_or_else(|| anyhow!("unknown scenario {name:?} (run `multitool trace` for the list)"))?;
    let t = run(setup);
    for entry in &t.stored {
        if json {
            println!("{}", harness_api::event::to_line(entry));
        } else {
            println!("{}", line(entry));
        }
    }
    if !json {
        println!("ends: {:?}; task: {:?}; {} messages, {} turns, asked {}", t.ends, t.state.task, t.state.messages.len(), t.state.turns, t.asked);
    }
    Ok(())
}

/// The live terminal UI on a fixture state. Same `AppState` and `draw` as `screen`; keys and
/// clicks go through `frontend_tui::input`, and the next screen's data comes from fixtures until
/// the core exists.
fn preview(name: &str, theme: &str, script: Option<&str>, handoff: Option<&std::path::Path>) -> Result<()> {
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
    let mut live = Live::new(script)?;
    // The welcome state is a new session: it is real. The other states are design fixtures.
    let mut state = match (handoff, fixture.name) {
        (Some(path), _) => live.restore(&reload::take_handoff(path)?),
        (None, "welcome") => live.fresh_session(),
        (None, "startup") => live.startup(),
        _ => (fixture.build)(),
    };
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
        // Only a change is worth a frame: an idle session must not spin on drawing.
        let mut dirty = true;
        loop {
            state.pictures = picker.is_some();
            live.connect_windows(&mut state);
            dirty |= live.receive(&mut state);
            dirty |= live.receive_daemon(&mut state);
            dirty |= live.tick_reload(&mut state, &current.id)?;
            if let frontend_tui::model::Mode::Session(w) = &mut state.mode {
                let now = chrono::Utc::now().timestamp_millis() as u64;
                dirty |= w.tabs.iter_mut().fold(false, |any, t| frontend_tui::live::tick_pulses(&mut t.view, now) | any);
            }
            // Input still queued (a burst of wheel events, fast typing) is handled first and
            // drawn once after, instead of one frame per event.
            if !event::poll(std::time::Duration::ZERO)? {
                if dirty {
                    dirty = false;
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
                }
                // The launch blink: redraw when the eyes change.
                let (eyes, next) = images::eyes_at(launched.elapsed().as_millis() as u64);
                if state.eyes != eyes {
                    state.eyes = eyes;
                    dirty = true;
                    continue;
                }
                // Wait for input, for the next blink frame, or for a session's next event.
                const SESSION_TICK_MS: u64 = 16;
                let wait = next.map_or(SESSION_TICK_MS, |n| n.saturating_sub(launched.elapsed().as_millis() as u64).min(SESSION_TICK_MS));
                if !event::poll(std::time::Duration::from_millis(wait))? {
                    continue;
                }
            }
            dirty = true;
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
                // Dragging a map pans it; the release ends the drag (or unfolds a ticket).
                Event::Mouse(m) if m.kind == MouseEventKind::Drag(MouseButton::Left) => {
                    input::drag(&mut state, m.column, m.row);
                    continue;
                }
                Event::Mouse(m) if m.kind == MouseEventKind::Up(MouseButton::Left) => {
                    input::release(&mut state);
                    continue;
                }
                Event::Mouse(m) if matches!(m.kind, MouseEventKind::ScrollUp | MouseEventKind::ScrollDown) => {
                    input::scroll(&mut state, if m.kind == MouseEventKind::ScrollUp { 3 } else { -3 });
                    continue;
                }
                _ => continue,
            };
            match outcome {
                Outcome::Quit => return Ok(()),
                Outcome::LeaveSplash => state = live.startup(),
                Outcome::OpenSession(i) => {
                    if let Some(opened) = live.open_recent(i) {
                        state = opened;
                    }
                }
                Outcome::Action(i) => {
                    let kind = match &state.mode {
                        Mode::Startup(v) => v.actions.get(i).map(|a| a.kind),
                        _ => None,
                    };
                    match kind {
                        Some(ActionKind::NewSession) => state = live.fresh_session(),
                        // Resume: the newest session of this folder, or a new one if none.
                        Some(ActionKind::Resume) => state = live.open_recent(0).unwrap_or_else(|| live.fresh_session()),
                        Some(ActionKind::Settings) => {
                            state = live.fresh_session();
                            if let Mode::Session(w) = &mut state.mode {
                                w.overlay = Some(frontend_tui::model::Overlay::Settings(frontend_tui::overlay::settings_for(w)));
                            }
                        }
                        // Settings, tour and help screens come in later stages.
                        _ => {}
                    }
                }
                Outcome::Stay | Outcome::OpenSettings => {}
            }
            live.send(&mut state);
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
