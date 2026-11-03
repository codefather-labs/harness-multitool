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

#[derive(clap::Subcommand)]
enum DebugAction {
    /// Print the OpenAPI contract of a debug router.
    Contract,
    /// Test a running router against the contract; the token comes from MULTITOOL_DEBUG_TOKEN.
    Check {
        url: String,
        /// Also send approve with the token, which moves a waiting pipeline on.
        #[arg(long)]
        approve: bool,
    },
    /// Draw the open window as it is now into a PNG, and print where. For debugging any screen.
    Screenshot {
        /// The file to write; a temporary one by default.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// The saved runs of the project in this directory, newest first.
    Runs {
        #[arg(long, default_value_t = 5)]
        limit: usize,
    },
}

#[derive(clap::Subcommand)]
enum DocsAction {
    /// Ids present, statuses matching, indexes complete; the prose is not judged.
    Check,
}

#[derive(Subcommand)]
enum JournalAction {
    /// Bring in a journal and maps from a stream of JSON documents, keeping their ids. Run again it
    /// updates what changed.
    Import {
        /// A file, or - for standard input.
        file: String,
        /// Move what is here out of the way first; it is renamed, not deleted.
        #[arg(long)]
        aside: bool,
    },
    /// Print the journal and maps as that stream.
    Export,
}

#[derive(Subcommand)]
enum Command {
    /// Open the terminal UI on a named state (default: welcome). F2 cycles themes; /exit or Ctrl+C twice quits, Ctrl+C alone stops a run (docs/tui.md).
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
    /// Sign in with a ChatGPT plan, on OpenAI's own page. Only `chatgpt` is offered: Anthropic does
    /// not permit Claude subscription sign-in in other apps, so Anthropic takes an API key.
    Login {
        /// `chatgpt`
        provider: String,
    },
    /// Sign out of a ChatGPT plan: the token is revoked and cleared.
    Logout {
        /// `chatgpt`
        provider: String,
    },
    /// Bring in a conversation another agent keeps for this directory; with no id, list them.
    /// The original is only read, and goes on in a fork (ADR 0022).
    Import {
        /// `claude-code`
        source: String,
        id: Option<String>,
    },
    /// This project's journal and maps, whole.
    Journal {
        #[command(subcommand)]
        action: JournalAction,
    },
    /// The harness's tools over MCP, on standard input and output: an agent backend starts this.
    #[command(hide = true)]
    Mcp {
        #[arg(long)]
        root: String,
        #[arg(long)]
        window: Option<String>,
        #[arg(long)]
        tab: Option<u64>,
        /// `mode`: serve the harness's modes alone.
        #[arg(long)]
        namespace: Option<String>,
    },
    /// Create what is missing of docs/ and .harness/, overwriting nothing.
    Init,
    /// The documentation: `check` compares the docs tree with itself and the journal.
    Docs {
        #[command(subcommand)]
        action: DocsAction,
    },
    /// Step-debug: the router's contract, its check, and the saved runs.
    Debug {
        #[command(subcommand)]
        action: DebugAction,
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
        /// The message for the agent; `-` reads it from stdin, which keeps it off the command line,
        /// where another program's `pkill -f` could match it.
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
        /// Start in plan mode: reads and plans, changes nothing.
        #[arg(long)]
        plan: bool,
        /// Start in step-debug mode.
        #[arg(long)]
        step_debug: bool,
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
        Command::Login { provider } => login(&provider),
        Command::Logout { provider } => {
            only_chatgpt(&provider)?;
            let store = harness_auth_chatgpt::Store::new(&config::data_dir()?);
            let confirmed = harness_auth_chatgpt::logout(&store).map_err(|e| anyhow!(e))?;
            println!("{}", if confirmed { "signed out" } else { "signed out here, but OpenAI did not confirm the revocation: disconnect the app in ChatGPT's settings" });
            Ok(())
        }
        Command::Import { source, id } => import_conversation(&source, id.as_deref()),
        Command::Journal { action } => {
            let root = std::env::current_dir()?.to_string_lossy().into_owned();
            let data = config::data_dir()?.to_string_lossy().into_owned();
            match action {
                JournalAction::Import { file, aside } => {
                    let text = if file == "-" { std::io::read_to_string(std::io::stdin())? } else { std::fs::read_to_string(&file).with_context(|| format!("reading {file}"))? };
                    let report = harness_session::journal_import(&data, &root, &text, aside).map_err(|e| anyhow!(e))?;
                    println!("entries: {} new, {} updated; maps: {}", report.new, report.updated, report.maps);
                    if let Some(aside) = report.aside {
                        println!("what was here is now {aside}");
                    }
                }
                JournalAction::Export => print!("{}", harness_session::journal_export(&data, &root).map_err(|e| anyhow!(e))?),
            }
            Ok(())
        }
        Command::Mcp { root, window, tab, namespace } => {
            // Each server reads the config file as it starts: the limit it holds is the one saved then (CON-114).
            let (map_max_nodes, broken) = config::map_limit();
            if let Some(broken) = broken {
                eprintln!("{broken}");
            }
            let serving = harness_session::mcp::Serving { data_dir: config::data_dir()?, root, window: window.zip(tab), namespace, map_max_nodes, search_max_linked: config::search_page() };
            harness_session::mcp::serve(&serving, std::io::stdin().lock(), std::io::stdout().lock()).map_err(|e| anyhow!(e))
        }
        Command::Init => {
            let root = std::env::current_dir()?.to_string_lossy().into_owned();
            let created = init_project(&root).map_err(|e| anyhow!(e))?;
            println!("created {} files", created.len());
            created.iter().for_each(|f| println!("{f}"));
            Ok(())
        }
        Command::Docs { action: DocsAction::Check } => docs_check(),
        Command::Debug { action: DebugAction::Screenshot { out } } => {
            let path = std::env::current_dir()?.join(out.unwrap_or_else(|| std::env::temp_dir().join(format!("multitool-screen-{}.png", std::process::id()))));
            // The window writes the file into this folder; if it is missing, no picture can come, so say that now.
            let Some(dir) = path.parent().filter(|dir| dir.is_dir()) else {
                bail!("the folder of {} does not exist", path.display());
            };
            // A folder the window cannot write into would only show as a 10 s timeout (journal#554): probe it now.
            let probe = dir.join(format!(".multitool-probe-{}", std::process::id()));
            if let Err(e) = std::fs::File::create_new(&probe) {
                bail!("cannot write the picture to {}: {e}", dir.display());
            }
            let _ = std::fs::remove_file(&probe);
            let client = harness_daemon::client::Client::find(&config::data_dir()?).ok_or_else(|| anyhow!("the daemon is not running, so no window is open"))?;
            let _ = std::fs::remove_file(&path);
            client.conversation_op(&serde_json::json!({"window": "-", "op": "screenshot", "text": path})).map_err(|e| anyhow!(e))?;
            let end = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while !path.exists() {
                if std::time::Instant::now() > end {
                    bail!("no window wrote the picture in 10 s");
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            println!("{}", path.display());
            Ok(())
        }
        Command::Debug { action: DebugAction::Contract } => {
            print!("{}", harness_daemon::stepdebug::CONTRACT);
            Ok(())
        }
        Command::Debug { action: DebugAction::Check { url, approve } } => {
            let token = std::env::var("MULTITOOL_DEBUG_TOKEN").ok().filter(|t| !t.is_empty());
            let report = harness_daemon::stepdebug::check_router(&url, token.as_deref(), approve);
            report.iter().for_each(|c| println!("{} {}", if c.ok { "ok  " } else { "FAIL" }, c.line));
            if report.iter().any(|c| !c.ok) {
                bail!("the router breaks the contract");
            }
            Ok(())
        }
        Command::Debug { action: DebugAction::Runs { limit } } => {
            let root = std::env::current_dir()?.to_string_lossy().into_owned();
            for run in harness_daemon::stepdebug::runs(&root, limit).map_err(|e| anyhow!(e))? {
                println!("run #{} map {} instance {} steps {}", run.id, run.map, run.instance, run.steps.len());
                run.steps.iter().for_each(|s| println!("  {} node {} {}{} {}", s.at_ms, s.node, s.state, if s.off_chart { " off-chart" } else { "" }, s.data));
            }
            Ok(())
        }
        Command::Run { prompt, yes, verify, task, plan, step_debug } => run_once(&prompt, yes, plan, step_debug, (task || !verify.is_empty()).then_some(verify)),
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
            daemon.telegram_from(std::sync::Arc::new(config::telegram));
            // The conversations run here, whichever window asks (ADR 0024).
            let make_host: harness_session::remote::MakeHost = std::sync::Arc::new(|settings, note, reload| host_for(note, reload, selection_summary(settings)));
            daemon.host_conversations(harness_session::remote::DaemonHost::new(&dir, make_host));
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
            let mut shown = c.clone();
            shown.telegram.token = shown.telegram.token.map(|_| "<hidden>".to_string());
            println!("{}\n{}", p.display(), toml::to_string_pretty(&shown)?);
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

/// How a speech model's download stands, between its thread and the window.
#[derive(Default)]
struct VoiceDownload {
    how_far: String,
    end: Option<Result<(), String>>,
}

/// The settings' test of speech to text, between the microphone, the engine and the page.
struct VoiceTestRun {
    /// On until it is stopped.
    recording: Option<harness_speech::Recording>,
    model: harness_speech::Model,
    heard: std::sync::Arc<std::sync::Mutex<Option<Result<String, String>>>>,
}

/// How the Telegram channel stands, in words, as the daemon says. Holds no token.
fn telegram_status() -> String {
    let stats = config::data_dir().ok().and_then(|d| harness_daemon::client::Client::find(&d)).and_then(|c| c.stats().ok());
    let Some(telegram) = stats.map(|s| s["telegram"].clone()) else { return "The daemon is not running.".to_string() };
    if telegram["on"] != true {
        return "Off: no token.".to_string();
    }
    match telegram["last_error"].as_str() {
        Some(error) => format!("On, with a problem: {error}"),
        None => format!("On. Chats bound: {}. Messages waiting: {}.", telegram["bound"], telegram["held"]),
    }
}

/// The window's state as a picture, at the sizes `multitool screen` uses. The file appears whole.
fn draw_to_file(state: &frontend_tui::AppState, theme: &Theme, cols: u16, rows: u16, path: &std::path::Path) -> Result<()> {
    let image = Raster::new(20.0, 26)?.render_with(state, theme, cols, rows, true);
    let partial = path.with_extension("partial.png");
    image.save(&partial)?;
    std::fs::rename(&partial, path)?;
    Ok(())
}

/// A log time as a card's stamp.
fn syncing_overlay(what: &str) -> frontend_tui::model::Overlay {
    let now_ms = chrono::Utc::now().timestamp_millis() as u64;
    frontend_tui::model::Overlay::Syncing(frontend_tui::model::SyncingView { what: what.to_string(), started_ms: now_ms, now_ms })
}

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
    harness_session::Host { note, plugins, reload, environment: None, window: None, choices: Default::default() }
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
        w.view_mut().status.fields = [("MODE", "normal"), ("ADAPTER", provider), ("MODEL", model), ("TOKENS", "0"), ("COST", "$0.00"), ("BUDGET", budget)]
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
/// A sign-in's outcome: the account and its models, or why it failed.
/// A provider row's models, or why there are none.
type ModelList = (String, Result<Vec<frontend_tui::model::ModelChoice>, String>);
type LoginOutcome = Result<(Option<String>, Vec<frontend_tui::model::ModelChoice>), String>;
type LoginEvents = (std::sync::mpsc::Sender<LoginOutcome>, std::sync::mpsc::Receiver<LoginOutcome>);

struct Live {
    /// The project's checks as last shown for approval.
    checks_shown: Option<String>,
    /// What each tab's plan usage would cost on the API, in micro-dollars.
    plan_cost: std::collections::HashMap<u64, u64>,
    /// Tabs to move to new settings when their run ends.
    rebind_after: std::collections::HashSet<u64>,
    toast_shown: Option<(String, std::time::Instant)>,
    /// Set while a browser sign-in waits: setting the flag cancels it.
    login_cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    login_events: LoginEvents,
    plan_models: (std::sync::mpsc::Sender<ModelList>, std::sync::mpsc::Receiver<ModelList>),
    step_debug_tabs: std::collections::HashSet<u64>,
    window_ids: std::collections::HashMap<u64, (harness_daemon::client::Client, String)>,
    first_start_done: bool,
    /// When the locked prompt on screen first showed.
    prompt_opened: Option<std::time::Instant>,
    sessions: harness_session::remote::SessionLink,
    /// Tabs opened on a run still on: entries up to this number came from the log.
    shown_up_to: std::collections::HashMap<u64, u64>,
    /// Messages typed in by a launcher, until there is a session to take them.
    held_inbound: Vec<(String, String)>,
    /// The user chose the context window: what a model reports does not change it (CON-80).
    context_chosen: bool,
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
    /// A speech model on its way, if one is.
    voice_download: Option<std::sync::Arc<std::sync::Mutex<VoiceDownload>>>,
    voice_test: Option<VoiceTestRun>,
    voice_devices: Option<std::sync::Arc<std::sync::Mutex<Option<harness_speech::InputDevices>>>>,
    /// The agent's `plugins_reload` tool raises this.
    reload_asked: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Tabs whose conversation the daemon is bringing in line: the window waits (CON-81).
    syncing: std::collections::HashSet<u64>,
    /// What to go back to if the change of adapter under way fails.
    previous_adapter: Option<(String, String)>,
    /// A conversation of Claude Code's being linked, off the window's thread.
    linking: Option<std::sync::mpsc::Receiver<Result<harness_session::SessionInfo, String>>>,
    /// The reload in hand was the user's own.
    reload_by_user: bool,
    /// The tab to tell how the reload in hand went.
    reload_for: Option<u64>,
    /// A rebuild in progress.
    reloading: Option<std::sync::mpsc::Receiver<Result<(), String>>>,
    /// Built; the swap waits for a safe point.
    ready_to_swap: bool,
    /// When the daemon was last asked to step aside for the swap.
    swap_tried: Option<std::time::Instant>,
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
            let Ok(program) = harness_daemon::own_program() else { continue };
            let Ok(client) = harness_daemon::client::Client::find_or_start(&dir, &program) else { continue };
            let Ok(window) = client.register(&self.root) else { continue };
            self.listen(tab, client, window);
        }
        // The daemon may have just started: the settings should say so.
        if self.windows.len() != before {
            w.callbacks = Some(self.callbacks_page());
            w.telegram = Some(self.telegram_page());
            w.voice = Some(self.voice_page());
            w.memory = Some(self.memory_page());
            w.auth = Some(self.auth_page(w.auth.clone()));
            self.fetch_models();
        }
    }

    /// Remembers a tab's window and hears its events from now on.
    fn listen(&mut self, tab: u64, client: harness_daemon::client::Client, window: harness_daemon::client::Registered) {
        if self.step_debug_tabs.contains(&tab) {
            let _ = client.set_step_debug(&window.window, true);
        }
        self.window_ids.insert(tab, (client.clone(), window.window.clone()));
        if let Ok(mut urls) = self.urls.lock() {
            urls.insert(tab, window.url.clone());
            drop(urls);
            if let Some(note) = callback_note(&self.urls, tab) {
                self.sessions.note(tab, note);
            }
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
        let client = match (config::data_dir(), harness_daemon::own_program()) {
            (Ok(dir), Ok(program)) => harness_daemon::client::Client::find_or_start(&dir, &program).ok(),
            _ => None,
        };
        let Mode::Session(w) = &mut state.mode else { return state };
        let template = w.view().clone();
        w.tabs.clear();
        for (index, saved) in handoff.tabs.iter().enumerate() {
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
                    if let harness_api::Event::StepDebug { on, .. } = entry.event {
                        self.note_step_debug(id, on);
                    }
                    feed.apply(view, &entry);
                    w.journal.extend(feed.take_known());
                }
                feed.replayed(view);
                self.feeds.insert(id, feed);
                frontend_tui::live::name_tab(w.tabs.last_mut().expect("just pushed"));
            }
            if let (Some(client), Some(window), Some(url)) = (&client, &saved.window, &saved.url) {
                self.windows.insert(id);
                self.listen(id, client.clone(), harness_daemon::client::Registered { window: window.clone(), url: url.clone() });
            }
            // After the URL is known: the session's agent is told it.
            if let Some(session) = &saved.session {
                self.sessions.resume(id, session);
                // It asked for this: it goes on by itself.
                if handoff.tell == Some(index) {
                    w.tabs[index].view.generating = true;
                    self.sessions.inbound(id, harness_api::HARNESS_LABEL.to_string(), "You asked for a reload: the harness has been rebuilt and restarted with your changes, and this conversation is here. Go on with what you were doing.".to_string());
                }
            }
        }
        w.active = handoff.active.min(w.tabs.len().saturating_sub(1));
        w.callbacks = Some(self.callbacks_page());
            w.telegram = Some(self.telegram_page());
            w.voice = Some(self.voice_page());
            w.memory = Some(self.memory_page());
        state
    }

    /// Moves a hot reload along: the build's result, then the swap at a safe point.
    /// The daemon saves runs only for a window whose session is in the mode. A failure here costs
    /// a saved run, not the session.
    fn note_step_debug(&mut self, tab: u64, on: bool) {
        if on {
            self.step_debug_tabs.insert(tab);
        } else {
            self.step_debug_tabs.remove(&tab);
        }
        if let Some((client, window)) = self.window_ids.get(&tab) {
            let _ = client.set_step_debug(window, on);
        }
    }

    /// The harness's own toasts go by themselves; a message from outside waits to be closed.
    fn tick_toast(&mut self, w: &mut frontend_tui::model::Workspace) -> bool {
        const SHOWN: std::time::Duration = std::time::Duration::from_secs(8);
        if !matches!(w.toast.as_ref().map(|t| &t.kind), Some(frontend_tui::model::InboundKind::System)) {
            self.toast_shown = None;
            return false;
        }
        let text = w.toast.as_ref().map(|t| t.text.clone()).unwrap_or_default();
        let shown = self.toast_shown.get_or_insert_with(|| (text.clone(), std::time::Instant::now()));
        if shown.0 != text {
            *shown = (text, std::time::Instant::now());
            return false;
        }
        if shown.1.elapsed() < SHOWN {
            return false;
        }
        w.toast = None;
        self.toast_shown = None;
        true
    }

    /// CON-44: the one time this project is asked about its docs tree.
    fn tick_first_start(&mut self, w: &mut frontend_tui::model::Workspace) {
        if std::mem::replace(&mut self.first_start_done, true) || !self.real {
            return;
        }
        let asks = config::data_dir().is_ok_and(|d| harness_session::first_start_asks(&d.to_string_lossy(), &self.root));
        if asks {
            let lines = vec!["This project has no docs/ and no .harness/. /init creates them, overwriting nothing, and the agent then keeps the documentation in step with its journal. This is asked only once; /init works any time.".to_string()];
            w.view_mut().transcript.push(frontend_tui::model::Item::Tips { label: "INITIALIZE THE PROJECT?".into(), lines });
        }
    }

    /// A prompt takes its answer only after this long on screen: see `PermissionView::locked`.
    fn tick_prompt(&mut self, w: &mut frontend_tui::model::Workspace) -> bool {
        const GRACE: std::time::Duration = std::time::Duration::from_millis(800);
        let Some(frontend_tui::model::Overlay::Permission(p)) = &mut w.overlay else {
            self.prompt_opened = None;
            return false;
        };
        if !p.locked {
            self.prompt_opened = None;
            return false;
        }
        if self.prompt_opened.get_or_insert_with(std::time::Instant::now).elapsed() < GRACE {
            return false;
        }
        p.locked = false;
        self.prompt_opened = None;
        true
    }

    fn tick_reload(&mut self, state: &mut frontend_tui::AppState, theme: &str) -> Result<bool> {
        use frontend_tui::model::{Item, Mode, Overlay};
        let Mode::Session(w) = &mut state.mode else { return Ok(false) };
        let note = |w: &mut frontend_tui::model::Workspace, text: String| {
            w.view_mut().transcript.push(Item::Tips { label: "RELOAD".into(), lines: text.lines().map(str::to_string).collect() });
        };
        let mut changed = false;
        if self.reload_asked.swap(false, std::sync::atomic::Ordering::SeqCst) && self.reloading.is_none() && !self.ready_to_swap {
            // The agent asked, unless the user did: then it hears how it went.
            let asked_by = self.sessions.reload_asked_by();
            self.reload_for = if std::mem::take(&mut self.reload_by_user) { None } else { Some(asked_by.unwrap_or(w.tabs[w.active].id)) };
            self.reloading = Some(reload::start_build());
            note(w, "Rebuilding…".to_string());
            changed = true;
        }
        if let Some(result) = self.reloading.as_ref().and_then(|r| r.try_recv().ok()) {
            self.reloading = None;
            changed = true;
            match result {
                Ok(()) => self.ready_to_swap = true,
                Err(message) => {
                    note(w, format!("Nothing was swapped: {message}"));
                    if let Some(tab) = self.reload_for.take() {
                        if let Some(t) = w.tabs.iter_mut().find(|t| t.id == tab) {
                            t.view.generating = true;
                        }
                        self.sessions.inbound(tab, harness_api::HARNESS_LABEL.to_string(), format!("The build you asked for failed, and nothing was swapped: the harness is as it was.\n{message}"));
                    }
                }
            }
        }
        // CON-50: never during a model turn or a tool call, never over a permission prompt.
        let busy = w.tabs.iter().any(|t| t.view.generating) || matches!(w.overlay, Some(Overlay::Permission(_))) || !self.asks.is_empty();
        if !self.ready_to_swap || busy {
            return Ok(changed);
        }
        if self.swap_tried.is_some_and(|at| at.elapsed() < std::time::Duration::from_millis(200)) {
            return Ok(changed);
        }
        self.swap_tried = Some(std::time::Instant::now());
        let dir = config::data_dir()?;
        let tell = self.reload_for.and_then(|tab| w.tabs.iter().position(|t| t.id == tab));
        // Before the daemon goes: it knows the tabs' sessions.
        let handoff = reload::Handoff {
            tell,
            active: w.active,
            theme: theme.to_string(),
            tabs: w
                .tabs
                .iter()
                .map(|t| reload::TabHandoff {
                    title: t.title.clone(),
                    session: self.sessions.session_of(t.id),
                    window: self.registered.get(&t.id).map(|r| r.window.clone()),
                    url: self.registered.get(&t.id).map(|r| r.url.clone()),
                    scroll: t.view.scroll,
                    draft: t.view.composer.text.clone(),
                })
                .collect(),
        };
        // The daemon holds the custom handlers, so it is replaced too; the new process starts it.
        // What this window shows can be behind it: a message may be starting there. It decides.
        if let Some(client) = harness_daemon::client::Client::find(&dir)
            && client.shutdown_if_idle() == Ok(false)
        {
            return Ok(changed);
        }
        let path = reload::write_handoff(&dir, &handoff)?;
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
                    if label == harness_api::TELEGRAM_LABEL {
                        w.toast = Some(frontend_tui::model::Toast { kind: frontend_tui::model::InboundKind::Telegram, from: "operator".into(), text: text.clone() });
                    }
                    w.tabs[index].view.generating = true;
                    self.sessions.inbound(tab, label, text);
                }
                WindowEvent::Journal { record } => {
                    let card = frontend_tui::live::journal_card(&record, &now);
                    w.journal.push(card.clone());
                    push_card(&mut w.tabs[index].view, Item::Journal(card), &now);
                }
                WindowEvent::Flow { map, instance, node, state, off_chart, path, run } => {
                    if let Some(run) = run {
                        let line = format!("Pass {instance} reached the end of {}: saved as run #{run} in .harness/debug/runs.sqlite, and the chart starts over.", map.title);
                        w.tabs[index].view.transcript.push(Item::Tips { label: "CYCLE SAVED".into(), lines: vec![line] });
                    }
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
        let context_chosen = settings.providers.get(&settings.provider).is_some_and(|row| row["context_chosen"] == true);
        let budget = settings.max_cost_usd.map_or("none".to_string(), |d| format!("${d:.2}"));
        let urls: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<u64, String>>> = Default::default();
        let reload_asked: std::sync::Arc<std::sync::atomic::AtomicBool> = Default::default();
        let link_settings = settings.clone();
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
        let local = harness_session::Sessions::new(factory, root.clone()).with_data_dir(&config::data_dir()?.to_string_lossy());
        // Everything goes through the daemon; a scripted run, or no daemon to be had, stays here.
        let remote = match script {
            Some(_) => None,
            None => harness_daemon::own_program().ok().and_then(|program| harness_session::remote::daemon_for_this_build(&config::data_dir().ok()?, &program).ok()).and_then(|client| harness_session::remote::RemoteSessions::open(client, &root, &link_settings, reload_asked.clone()).ok()),
        };
        Ok(Live { sessions: remote.map_or(harness_session::remote::SessionLink::Local(local), harness_session::remote::SessionLink::Remote), syncing: Default::default(), previous_adapter: None, linking: None, reload_by_user: false, reload_for: None, shown_up_to: Default::default(), held_inbound: Vec::new(), context_chosen, checks_shown: None, plan_cost: Default::default(), rebind_after: Default::default(), toast_shown: None, login_cancel: None, login_events: std::sync::mpsc::channel(), plan_models: std::sync::mpsc::channel(), step_debug_tabs: Default::default(), window_ids: Default::default(), prompt_opened: None, first_start_done: false, feeds: Default::default(), asks: Default::default(), context_window, root, real: script.is_none(), recent: Vec::new(), provider, model, budget, urls, windows: Default::default(), window_events: std::sync::mpsc::channel(), registered: Default::default(), reload_asked, reloading: None, ready_to_swap: false, swap_tried: None, voice_download: None, voice_test: None, voice_devices: None })
    }

    fn fresh_session(&self) -> frontend_tui::AppState {
        let mut state = live_session(&self.provider, &self.model, &self.budget);
        if let frontend_tui::model::Mode::Session(w) = &mut state.mode {
            w.callbacks = Some(self.callbacks_page());
            w.telegram = Some(self.telegram_page());
            w.voice = Some(self.voice_page());
            w.memory = Some(self.memory_page());
            if w.auth.is_none() {
                w.auth = Some(self.auth_page(None));
            }
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

    fn voice_page(&self) -> frontend_tui::model::VoicePage {
        let voice = config::load().map(|c| c.voice).unwrap_or_default();
        let data = config::data_dir().ok();
        let chosen = voice.chosen();
        let models = harness_speech::MODELS.iter().map(|m| frontend_tui::model::VoiceModel { engine: m.engine.id().to_string(), name: m.name.to_string(), size: m.size.to_string(), there: data.as_ref().is_some_and(|d| harness_speech::model_present(d, *m)) }).collect();
        let runtime_size = (!data.as_ref().is_some_and(|d| harness_speech::runtime_present(d))).then(|| harness_speech::RUNTIME_SIZE.to_string());
        let device = voice.device.clone().map(|id| frontend_tui::model::VoiceDevice { name: voice.device_name.clone().unwrap_or_else(|| id.clone()), id });
        frontend_tui::model::VoicePage { engine: chosen.map(|m| m.engine.id().to_string()), model: chosen.map_or(String::new(), |m| m.name.to_string()), models, runtime_size, device, ..Default::default() }
    }

    /// Moves a speech download along on the page.
    fn tick_voice_download(&mut self, w: &mut frontend_tui::model::Workspace) -> bool {
        let Some(progress) = &self.voice_download else { return false };
        let (how_far, end) = {
            let mut p = progress.lock().expect("download");
            (p.how_far.clone(), p.end.take())
        };
        if end.is_none() {
            let Some(frontend_tui::model::Overlay::Settings(s)) = &mut w.overlay else { return false };
            let changed = s.voice.downloading.as_deref() != Some(how_far.as_str());
            s.voice.downloading = Some(how_far);
            return changed;
        }
        self.voice_download = None;
        // A channel that started without it hears notes from now on.
        if let Some(client) = config::data_dir().ok().and_then(|d| harness_daemon::client::Client::find(&d)) {
            let _ = client.telegram_reload();
        }
        let fresh = self.voice_page();
        if let Some(frontend_tui::model::Overlay::Settings(s)) = &mut w.overlay {
            s.voice.models = fresh.models.clone();
            s.voice.runtime_size = fresh.runtime_size.clone();
            s.voice.downloading = None;
            s.voice.status = match end {
                Some(Ok(())) => "Downloaded.".to_string(),
                Some(Err(why)) => why,
                None => String::new(),
            };
        }
        w.voice = Some(fresh);
        true
    }

    /// What the system listed reaches the open dropdown.
    fn tick_voice_devices(&mut self, w: &mut frontend_tui::model::Workspace) -> bool {
        let Some(asked) = &self.voice_devices else { return false };
        let Some(found) = asked.lock().expect("devices").take() else { return false };
        self.voice_devices = None;
        let Some(frontend_tui::model::Overlay::Settings(s)) = &mut w.overlay else { return false };
        s.voice.devices = found.list.into_iter().map(|d| frontend_tui::model::VoiceDevice { id: d.id, name: d.name }).collect();
        s.voice.default_device = found.default;
        // The list may have got shorter under the highlight.
        s.voice.menu = s.voice.menu.map(|at| at.min(s.voice.devices.len()));
        true
    }

    /// The settings' test: the level while the microphone is on, then the words.
    fn tick_voice_test(&mut self, w: &mut frontend_tui::model::Workspace) -> bool {
        use frontend_tui::model::VoiceTest;
        let Some(test) = &mut self.voice_test else { return false };
        let Some(frontend_tui::model::Overlay::Settings(s)) = &mut w.overlay else {
            // The settings closed: the microphone goes off with them.
            self.voice_test = None;
            return false;
        };
        if let Some(recording) = &test.recording {
            if !recording.finished() {
                let now = VoiceTest::Listening { level: recording.level(), seconds: recording.seconds() };
                let changed = s.voice.test != now;
                // A press of Enter is on its way here as a stop, not to be drawn over.
                if matches!(s.voice.test, VoiceTest::Listening { .. }) {
                    s.voice.test = now;
                }
                return changed;
            }
            s.voice.test = VoiceTest::Hearing;
            self.stop_voice_test();
            return true;
        }
        let Some(result) = test.heard.lock().expect("heard").take() else { return false };
        self.voice_test = None;
        match result {
            Ok(words) => s.voice.test = VoiceTest::Heard(words),
            Err(why) => {
                s.voice.test = VoiceTest::Idle;
                s.voice.status = why;
            }
        }
        true
    }

    /// The microphone off, and what it heard to the engine, off the window's thread.
    fn stop_voice_test(&mut self) {
        let Some(test) = &mut self.voice_test else { return };
        let Some(recording) = test.recording.take() else { return };
        let (model, heard) = (test.model, test.heard.clone());
        let Ok(data) = config::data_dir() else { return };
        std::thread::spawn(move || {
            let samples = recording.stop();
            let result = harness_speech::transcribe_samples(&data, model, &samples);
            *heard.lock().expect("heard") = Some(result);
        });
    }

    fn memory_page(&self) -> frontend_tui::model::MemoryPage {
        let (map_max_nodes, broken) = config::map_limit();
        frontend_tui::model::MemoryPage { map_max_nodes, search_max_linked: config::search_page(), status: broken.unwrap_or_default(), ..Default::default() }
    }

    fn telegram_page(&self) -> frontend_tui::model::TelegramPage {
        let file = config::load().map(|c| c.telegram).unwrap_or_default();
        frontend_tui::model::TelegramPage { token_saved: file.token.is_some(), users: file.allowed_users.iter().map(i64::to_string).collect::<Vec<_>>().join(", "), status: telegram_status(), ..Default::default() }
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
    /// The sign-in state as things stand, keeping what the user is doing on it.
    fn auth_page(&self, keep: Option<frontend_tui::model::AuthPage>) -> frontend_tui::model::AuthPage {
        let mut page = keep.unwrap_or_default();
        let session = config::load().map(|c| c.session).unwrap_or_default();
        let keys = harness_auth_apikey::ApiKeyAuth::new(|name| std::env::var(name).ok(), &serde_json::json!(session.auth));
        page.keys = ["Anthropic API", "OpenAI-compatible", "Local"].iter().map(|row| (row.to_string(), keys.source(&provider_id(row)))).collect();
        page.delegate = cfg!(feature = "claude-code");
        // A row not there yet starts in the user's own environment, asking permissions.
        page.delegate_row = session.providers.get("claude-code").map_or((true, false), |row| (row["environment"] == "user", row["permissions"] == "skip"));
        page.contexts = session.providers.iter().filter(|(_, row)| row["context_chosen"] == true).filter_map(|(id, row)| Some((id.clone(), row["context_window"].as_u64()?))).collect();
        if let Ok(dir) = config::data_dir() {
            (page.signed_in, page.plan_status) = harness_auth_chatgpt::status(&harness_session_store(&dir));
        }
        page.pending = self.login_cancel.is_some();
        page
    }

    /// Changes the sign-in state both where it is drawn and where it is kept.
    fn update_auth(&self, w: &mut frontend_tui::model::Workspace, change: impl FnOnce(&mut frontend_tui::model::AuthPage)) {
        use frontend_tui::model::Overlay;
        let open = match &w.overlay {
            Some(Overlay::Settings(s)) => Some(s.auth.clone()),
            _ => w.auth.clone(),
        };
        let mut page = self.auth_page(open);
        change(&mut page);
        if let Some(Overlay::Settings(s)) = &mut w.overlay {
            s.auth = page.clone();
        }
        w.auth = Some(page);
    }

    /// What was saved reaches the open tabs: each session goes on with its history under the new
    /// settings, a running one once its run ends. The status line says so at once, which is also
    /// where the settings window reads the current choice from.
    /// A tab's conversation goes on this adapter and model (CON-94); the others stay as they are.
    fn apply_to_tab(&mut self, w: &mut frontend_tui::model::Workspace, tab: u64, provider: &str, model: &str) {
        if let Some(feed) = self.feeds.get_mut(&tab) {
            feed.set_context_window(self.context_window);
        }
        let Some(t) = w.tabs.iter_mut().find(|t| t.id == tab) else { return };
        frontend_tui::live::set_status(&mut t.view, "ADAPTER", provider);
        frontend_tui::live::set_status(&mut t.view, "MODEL", model);
        frontend_tui::live::show_context_window(&mut t.view, self.context_window);
        let has_session = self.sessions.session_of(tab).is_some();
        match self.sessions.choose(tab, provider, model) {
            Ok(true) if has_session => {
                self.syncing.insert(tab);
            }
            Ok(_) => {}
            Err(why) => t.view.transcript.push(frontend_tui::model::Item::Tips { label: "SETTINGS".into(), lines: vec![why] }),
        }
        if !self.syncing.is_empty() && w.overlay.is_none() {
            w.overlay = Some(syncing_overlay("Bringing the conversation in line"));
        }
    }

    /// The gpt-subscription pipeline: a listener, the browser, a thread that waits for the
    /// callback. Its outcome comes back through `tick_login`.
    fn start_login(&mut self, w: &mut frontend_tui::model::Workspace) {
        if self.login_cancel.is_some() {
            return;
        }
        let Ok(dir) = config::data_dir() else { return };
        let session = config::load().map(|c| c.session).unwrap_or_default();
        let issuer = session.auth.get("chatgpt").and_then(|a| a["issuer"].as_str()).unwrap_or(harness_auth_chatgpt::ISSUER).to_string();
        let base = plan_base_url(&session);
        let store = harness_session_store(&dir);
        let pending = match harness_auth_chatgpt::begin_login(&store, &issuer, 1455) {
            Ok(pending) => pending,
            Err(e) => {
                self.update_auth(w, |a| a.message = vec![format!("Could not start the sign-in: {e}")]);
                return;
            }
        };
        let opened = open_browser(&pending.url);
        let url = pending.url.clone();
        let cancel: std::sync::Arc<std::sync::atomic::AtomicBool> = Default::default();
        self.login_cancel = Some(cancel.clone());
        let sender = self.login_events.0.clone();
        std::thread::spawn(move || {
            let outcome = pending.finish_or_cancel(&store, std::time::Duration::from_secs(600), &cancel);
            let event = outcome.map(|credential| {
                let models = harness_auth_chatgpt::list_models(&base, &credential.access_token).map(|m| choices(frontend_tui::model::PLAN_PROVIDER, m)).unwrap_or_default();
                (credential.email, models)
            });
            let _ = sender.send(event);
        });
        self.update_auth(w, |a| {
            a.pending = true;
            let first = if opened { "Sign in on the page that opened. If none did, open this:" } else { "Open this in your browser and sign in:" };
            let kept = format!("The whole address is also in {}", harness_auth_chatgpt::login_url_path(&harness_session_store(&dir)).display());
            a.message = [first.to_string(), url, String::new(), kept].into_iter().chain(harness_auth_chatgpt::remote_hint(1455)).collect();
        });
    }

    /// Each provider's models, in the background, for the Provider page. A server that is down
    /// or a key that is missing costs its list, and the page says so only for the chosen one.
    fn fetch_models(&mut self) {
        let Ok(dir) = config::data_dir() else { return };
        let session = config::load().map(|c| c.session).unwrap_or_default();
        let sender = self.plan_models.0.clone();
        std::thread::spawn(move || {
            let plan = harness_auth_chatgpt::ChatGptAuth::new(&dir);
            if harness_auth_chatgpt::status(&harness_session_store(&dir)).0 {
                let answer = pollster::block_on(harness_api::Auth::credentials(&plan, "chatgpt")).map_err(|e| e.to_string()).and_then(|token| harness_auth_chatgpt::list_models(&plan_base_url(&session), token.expose()));
                let _ = sender.send((frontend_tui::model::PLAN_PROVIDER.to_string(), answer.map(|m| choices(frontend_tui::model::PLAN_PROVIDER, m))));
            }
            let keys = harness_auth_apikey::ApiKeyAuth::new(|name| std::env::var(name).ok(), &serde_json::json!(session.auth));
            if let Ok(key) = pollster::block_on(harness_api::Auth::credentials(&keys, "anthropic")) {
                let base = session.providers.get("anthropic").and_then(|p| p["base_url"].as_str()).map_or("https://api.anthropic.com/v1".to_string(), |b| format!("{}/v1", b.trim_end_matches('/').trim_end_matches("/v1")));
                let answer = harness_auth_chatgpt::list_anthropic_models(&base, key.expose(), harness_provider_anthropic::API_VERSION);
                let _ = sender.send(("Anthropic API".to_string(), answer.map(|m| choices("Anthropic API", m))));
            }
            for row in ["OpenAI-compatible", "Local"] {
                let id = provider_id(row);
                let token = pollster::block_on(harness_api::Auth::credentials(&keys, &id)).map(|k| k.expose().to_string()).ok();
                // With no key there is nothing to ask a paid API; a local server needs none.
                let Some(token) = token.or((id == "local").then(String::new)) else { continue };
                let answer = harness_auth_chatgpt::list_models(&provider_base_url(&session, &id), &token);
                let _ = sender.send((row.to_string(), answer.map(|m| choices(row, m))));
            }
        });
    }

    /// A sign-in that ended. True if something changed.
    fn tick_login(&mut self, w: &mut frontend_tui::model::Workspace) -> bool {
        let mut changed = false;
        while let Ok((row, answer)) = self.plan_models.1.try_recv() {
            changed = true;
            let on_plan = row == frontend_tui::model::PLAN_PROVIDER;
            self.update_auth(w, |a| match answer {
                Ok(models) if on_plan => a.models = models,
                Ok(models) => {
                    a.provider_models.retain(|(p, _)| *p != row);
                    a.provider_models.push((row.clone(), models));
                }
                Err(reason) => a.message = vec![format!("{row}: {reason}")],
            });
            if let Some(frontend_tui::model::Overlay::Settings(s)) = &mut w.overlay {
                frontend_tui::overlay::fit_model(s);
            }
        }
        let Ok(event) = self.login_events.1.try_recv() else { return changed };
        self.login_cancel = None;
        let told = match &event {
            Ok(_) => "Signed in to ChatGPT.".to_string(),
            Err(reason) => reason.chars().take(90).collect(),
        };
        w.toast = Some(frontend_tui::overlay::system_toast(&told));
        // Signing in says what is wanted: the plan is put to use, with its first model, at once.
        let use_plan = event.as_ref().ok().and_then(|(_, models)| models.first().map(|m| m.id.clone()));
        let plan_context = event.as_ref().ok().and_then(|(_, models)| models.first().and_then(|m| m.context));
        let (mut applied, email) = (false, event.as_ref().ok().and_then(|(email, _)| email.clone()));
        if let Some(model) = &use_plan
            && self.save_settings("chatgpt", model, plan_context, config::load().ok().and_then(|c| c.session.providers.get("chatgpt").filter(|row| row["context_chosen"] == true).and_then(|row| row["context_window"].as_u64()))).is_ok()
        {
            let tab = w.tabs[w.active].id;
            self.apply_to_tab(w, tab, "chatgpt", model);
            applied = true;
        }
        self.update_auth(w, |a| match event {
            Ok((_, models)) => {
                let who = email.map_or(String::new(), |e| format!(" as {e}"));
                a.message = vec![if applied { format!("Signed in{who}, and in use now. Choose another model below if you like; Ctrl+S saves it.") } else { format!("Signed in{who}. Its models could not be listed.") }];
                a.models = models;
            }
            Err(reason) => a.message = vec![reason],
        });
        let _ = std::fs::remove_file(config::data_dir().map(|d| harness_auth_chatgpt::login_url_path(&harness_session_store(&d))).unwrap_or_default());
        if let Some(frontend_tui::model::Overlay::Settings(s)) = &mut w.overlay {
            frontend_tui::overlay::fit_model(s);
            if applied {
                if let Some(i) = s.providers.iter().position(|p| p.name == frontend_tui::model::PLAN_PROVIDER) {
                    s.provider = i;
                }
                s.saved = (s.provider, use_plan.clone().unwrap_or_default());
                if let Some(model) = use_plan {
                    s.model = model;
                }
            }
        }
        true
    }

    /// Saves a typed key and rebuilds the sessions' factory. Returns the provider's id.
    fn save_key(&mut self, row: &str, key: &harness_api::Secret) -> Result<String> {
        let id = provider_id(row);
        let mut c = config::load()?;
        let entry = c.session.auth.entry(id.clone()).or_insert_with(|| serde_json::json!({}));
        entry["key"] = serde_json::json!(key.expose());
        config::save(&c)?;
        self.rebuild_factory(c.session)?;
        Ok(id)
    }

    fn rebuild_factory(&mut self, session: harness_session::Settings) -> Result<()> {
        if self.real {
            let note = {
                let urls = self.urls.clone();
                std::sync::Arc::new(move |tab: u64| callback_note(&urls, tab))
            };
            let host = host_for(note, self.reload_asked.clone(), selection_summary(&session));
            let (root, data) = (self.root.clone(), config::data_dir()?);
            let for_link = session.clone();
            self.sessions.configure(&root, &for_link, || harness_session::with_host(session, root.clone(), data, host)).map_err(|e| anyhow!(e))?;
        }
        Ok(())
    }

    fn save_settings(&mut self, provider: &str, model: &str, context: Option<u64>, chosen: Option<u64>) -> Result<()> {
        self.save_settings_with(provider, model, context, chosen, None)
    }

    fn save_settings_with(&mut self, provider: &str, model: &str, context: Option<u64>, chosen: Option<u64>, delegate: Option<(bool, bool)>) -> Result<()> {
        let mut c = config::load()?;
        c.session.provider = provider.to_string();
        c.session.model = model.to_string();
        if provider == "claude-code" {
            c.session.providers.entry("claude-code".to_string()).or_insert_with(|| serde_json::json!({"kind": "claude-code", "context_window": 200_000}));
        }
        if provider == "local" {
            c.session.providers.entry("local".to_string()).or_insert_with(|| serde_json::json!({"kind": "openai", "base_url": LOCAL_BASE_URL}));
            c.session.auth.entry("local".to_string()).or_insert_with(|| serde_json::json!({"none": true}));
        }
        if provider == "chatgpt" {
            c.session.providers.entry("chatgpt".to_string()).or_insert_with(|| serde_json::json!({"kind": "openai-responses"}));
            c.session.auth.entry("chatgpt".to_string()).or_insert_with(|| serde_json::json!({"oauth": "chatgpt"}));
        }
        // What the server said this model holds: TOKENS and compaction go by it.
        // How the user has Claude Code run: their word alone (CON-70).
        if let (Some((own, skip)), Some(object)) = (delegate, c.session.providers.get_mut("claude-code").and_then(|row| row.as_object_mut())) {
            if own {
                object.insert("environment".to_string(), serde_json::json!("user"));
            } else {
                object.remove("environment");
            }
            if skip {
                object.insert("permissions".to_string(), serde_json::json!("skip"));
            } else {
                object.remove("permissions");
            }
        }
        // Or what the user chose, which outranks it (CON-80).
        let row = c.session.providers.entry(provider.to_string()).or_insert_with(|| serde_json::json!({}));
        if let Some(object) = row.as_object_mut() {
            match (chosen, context) {
                (Some(window), _) => {
                    object.insert("context_window".to_string(), serde_json::json!(window));
                    object.insert("context_chosen".to_string(), serde_json::json!(true));
                }
                (None, known) => {
                    if object.remove("context_chosen").is_some() && known.is_none() {
                        object.remove("context_window");
                    }
                    if let Some(window) = known {
                        object.insert("context_window".to_string(), serde_json::json!(window));
                    }
                }
            }
        }
        if self.real {
            let context_window = c.session.context_window();
            let note = {
                let urls = self.urls.clone();
                std::sync::Arc::new(move |tab: u64| callback_note(&urls, tab))
            };
            let host = host_for(note, self.reload_asked.clone(), selection_summary(&c.session));
            let (root, data) = (self.root.clone(), config::data_dir()?);
            let for_link = c.session.clone();
            let session = c.session.clone();
            self.sessions.configure(&root, &for_link, || harness_session::with_host(session, root.clone(), data, host)).map_err(|e| anyhow!(e))?;
            self.context_window = context_window;
            self.context_chosen = chosen.is_some();
            self.provider = provider.to_string();
            self.model = model.to_string();
        }
        config::save(&c)?;
        Ok(())
    }

    /// The startup page, listing this folder's real sessions.
    fn startup(&mut self) -> frontend_tui::AppState {
        let mut state = fixtures::startup();
        state.header.cwd = shown_cwd();
        state.header.branch = git_branch();
        self.recent = config::data_dir().map(|d| harness_session::recent(&d.to_string_lossy(), &self.root)).unwrap_or_default();
        // Claude Code's conversations of this folder too, those not linked already, or that went on
        // in Claude Code past a copy from before links.
        if let Ok(conversations) = harness_session::conversation_source("claude-code") {
            let linked: Vec<String> = self.recent.iter().filter(|s| s.linked).filter_map(|s| s.imported.clone()).collect();
            for c in conversations.list(&self.root).unwrap_or_default() {
                if !linked.contains(&c.id) {
                    let title = if c.title.is_empty() { c.id.chars().take(8).collect() } else { c.title.clone() };
                    self.recent.push(harness_session::SessionInfo { id: c.id, title, model: "claude-code".to_string(), last_ms: c.last_ms, imported: None, linked: false, foreign: true });
                }
            }
            self.recent.sort_by_key(|s| std::cmp::Reverse(s.last_ms));
        }
        if !self.real {
            self.recent.clear();
        }
        if let frontend_tui::model::Mode::Startup(v) = &mut state.mode {
            if self.real {
                for (name, value) in &mut v.status.fields {
                    match name.as_str() {
                        "ADAPTER" => value.clone_from(&self.provider),
                        "MODEL" => value.clone_from(&self.model),
                        _ => {}
                    }
                }
            }
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
        // A conversation of Claude Code's: linked, one conversation from here on (INV-21). It can
        // be gigabytes, so off this thread, and the window waits (CON-81).
        if info.foreign {
            let (data, root) = (config::data_dir().ok()?, self.root.clone());
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let linked = (|| -> Result<harness_session::SessionInfo, String> {
                    let imported = harness_session::conversation_source("claude-code")?.read(&root, &info.id).map_err(|e| e.to_string())?;
                    let now_ms = chrono::Utc::now().timestamp_millis() as u64;
                    let id = harness_session::store_imported(&data.to_string_lossy(), &root, &imported, now_ms)?;
                    // Linked needs the adapter known, whichever one is chosen now.
                    if let Ok(mut c) = config::load()
                        && !c.session.providers.contains_key("claude-code")
                    {
                        c.session.providers.insert("claude-code".to_string(), serde_json::json!({"kind": "claude-code", "environment": "user"}));
                        let _ = config::save(&c);
                    }
                    Ok(harness_session::SessionInfo { id, foreign: false, ..info })
                })();
                let _ = tx.send(linked);
            });
            self.linking = Some(rx);
            let mut state = self.fresh_session();
            if let frontend_tui::model::Mode::Session(w) = &mut state.mode {
                w.overlay = Some(syncing_overlay("Bringing in the conversation"));
            }
            return Some(state);
        }
        self.open_stored(info)
    }

    /// A conversation that was being linked is ready: it opens. Otherwise the window goes on.
    fn tick_linking(&mut self, state: &mut frontend_tui::AppState) {
        let Some(done) = self.linking.as_ref().and_then(|rx| rx.try_recv().ok()) else { return };
        self.linking = None;
        match done {
            Ok(info) => {
                if let Some(opened) = self.open_stored(info) {
                    *state = opened;
                }
            }
            Err(message) => {
                if let frontend_tui::model::Mode::Session(w) = &mut state.mode {
                    w.overlay = None;
                    w.view_mut().transcript.push(frontend_tui::model::Item::Tips { label: "COULD NOT BRING IT IN".into(), lines: message.lines().map(str::to_string).collect() });
                }
            }
        }
    }

    fn open_stored(&mut self, info: harness_session::SessionInfo) -> Option<frontend_tui::AppState> {
        let data = config::data_dir().ok()?;
        let mut state = self.fresh_session();
        if let frontend_tui::model::Mode::Session(w) = &mut state.mode {
            let tab = w.tabs[w.active].id;
            // Opened first and read after: what a run on says from here comes as events, and all
            // before is in the log. What both have shows once, by its number.
            let opened = self.sessions.resume(tab, &info.id);
            // What was sent and did not go in comes back to the composer, for the user to send.
            let left = self.sessions.left_waiting(tab, &info.id);
            let log = harness_session::read_log(&data.to_string_lossy(), &self.root, &info.id);
            let mut feed = frontend_tui::live::Feed::new(local_time, self.context_window);
            let view = w.view_mut();
            view.transcript.clear();
            for entry in &log {
                if let harness_api::Event::StepDebug { on, .. } = entry.event {
                    self.note_step_debug(tab, on);
                }
                if self.context_chosen && matches!(entry.event, harness_api::Event::ContextWindow { .. }) {
                    continue;
                }
                feed.apply(view, entry);
            }
            if opened == harness_session::remote::Opened::Attached {
                view.generating = true;
            } else {
                feed.replayed(view);
            }
            self.shown_up_to.insert(tab, log.last().map_or(0, |e| e.seq));
            let typed: Vec<&str> = left.iter().filter_map(|m| if let harness_api::PendingMessage::User { text } = m { Some(text.as_str()) } else { None }).collect();
            view.composer.text.push_str(&typed.join("\n\n"));
            for message in &left {
                if let harness_api::PendingMessage::Inbound { label, text } = message {
                    self.sessions.inbound(tab, label.clone(), text.clone());
                }
            }
            // What the next message goes through, not what the last one went through.
            let choice = self.sessions.choice_of(tab);
            frontend_tui::live::set_status(view, "ADAPTER", choice.as_ref().map_or(&self.provider, |c| &c.provider));
            frontend_tui::live::set_status(view, "MODEL", choice.as_ref().map_or(&self.model, |c| &c.model));
            if let Some(choice) = &choice {
                let window = config::load().map_or(self.context_window, |c| harness_session::Settings { provider: choice.provider.clone(), ..c.session }.context_window());
                feed.set_context_window(window);
                frontend_tui::live::show_context_window(view, window);
            }
            self.feeds.insert(tab, feed);
            let active = w.active;
            frontend_tui::live::name_tab(&mut w.tabs[active]);
            if opened == harness_session::remote::Opened::Ready {
                self.syncing.insert(tab);
                w.overlay = Some(syncing_overlay("Catching up with the conversation"));
            }
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
                    use harness_session::TaskRefused;
                    let refused = self.sessions.task(tab, text.clone(), steps.clone()).err();
                    let Some(t) = w.tabs.iter_mut().find(|t| t.id == tab) else { continue };
                    match refused {
                        None => t.view.generating = true,
                        Some(TaskRefused::Invalid(reason)) => t.view.transcript.push(frontend_tui::model::Item::Tips { label: "TASK REFUSED".into(), lines: vec![reason] }),
                        Some(TaskRefused::NotApproved { commands, text: file }) => {
                            // Nothing is lost: the message and its steps wait for the answer.
                            self.checks_shown = Some(file);
                            t.view.task = Some(steps);
                            t.view.composer.text = text;
                            let mut lines = vec!["This project's .harness/acceptance.toml is new or changed. A task would run on this machine:".to_string()];
                            lines.extend(commands.iter().map(|c| format!("$ {c}")));
                            lines.push("/task trust approves exactly this, then Enter sends your message. /task off cancels.".to_string());
                            t.view.transcript.push(frontend_tui::model::Item::Tips { label: "APPROVE THE CHECKS".into(), lines });
                        }
                    }
                }
                Outgoing::Login => self.start_login(w),
                Outgoing::LoginCancel => {
                    if let Some(cancel) = &self.login_cancel {
                        cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                }
                Outgoing::Logout => {
                    let line = match config::data_dir().map(|d| harness_session_store(&d)).map_err(|e| e.to_string()).and_then(|s| harness_auth_chatgpt::logout(&s)) {
                        Ok(true) => "Signed out, and the token is revoked.".to_string(),
                        Ok(false) => "Signed out here, but OpenAI did not confirm the revocation: disconnect the app in ChatGPT's settings.".to_string(),
                        Err(e) => format!("Could not sign out: {e}"),
                    };
                    self.update_auth(w, |a| {
                        a.message = vec![line];
                        a.models.clear();
                    });
                }
                Outgoing::SaveKey { provider, key } => {
                    let saved = self.save_key(&provider, &key);
                    if saved.is_ok() {
                        self.fetch_models();
                        for t in &w.tabs {
                            if t.view.generating || !self.sessions.rebind(t.id) {
                                self.rebind_after.insert(t.id);
                            }
                        }
                    }
                    let line = match saved {
                        Ok(id) => format!("Saved for {id} in the configuration, readable only by you. New sessions use it; a key in the environment still wins."),
                        Err(e) => format!("Could not save the key: {e}"),
                    };
                    w.toast = Some(frontend_tui::overlay::system_toast(if line.starts_with("Saved") { "The key is saved." } else { "The key was not saved." }));
                    self.update_auth(w, |a| a.message = vec![line]);
                }
                Outgoing::Plan { tab, on } => self.sessions.plan(tab, on),
                Outgoing::StepDebug { tab, on } => self.sessions.step_debug(tab, on),
                Outgoing::Nick { tab, name } => self.sessions.nick(tab, name),
                Outgoing::Unqueue { tab, text } => {
                    self.sessions.unqueue(tab, &text);
                }
                Outgoing::Init { tab } => {
                    let lines = match init_project(&self.root) {
                        Ok(created) if created.is_empty() => vec!["Nothing to do: the docs tree and .harness/ are already here.".to_string()],
                        Ok(created) => std::iter::once(format!("Created {} files:", created.len())).chain(created).collect(),
                        Err(e) => vec![format!("Could not initialize: {e}")],
                    };
                    if let Some(t) = w.tabs.iter_mut().find(|t| t.id == tab) {
                        t.view.transcript.push(frontend_tui::model::Item::Tips { label: "INIT".into(), lines });
                    }
                }
                Outgoing::ApproveChecks { tab } => {
                    let line = match self.checks_shown.take() {
                        None => "No checks are waiting for approval.".to_string(),
                        Some(file) => match config::data_dir().map_err(|e| e.to_string()).and_then(|d| harness_session::approve_acceptance(&d.to_string_lossy(), &self.root, &file)) {
                            Ok(()) => "Approved. If the file changes, a task asks again.".to_string(),
                            Err(e) => format!("Not approved: {e}"),
                        },
                    };
                    if let Some(t) = w.tabs.iter_mut().find(|t| t.id == tab) {
                        t.view.transcript.push(frontend_tui::model::Item::Tips { label: "CHECKS".into(), lines: vec![line] });
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
                Outgoing::SaveTelegram { token, users } => {
                    let saved = config::load().and_then(|mut c| {
                        if let Some(token) = token {
                            c.telegram.token = Some(token);
                        }
                        c.telegram.allowed_users = users;
                        config::save(&c)
                    });
                    let reloaded = saved.map_err(|e| e.to_string()).and_then(|_| {
                        let client = config::data_dir().ok().and_then(|d| harness_daemon::client::Client::find(&d)).ok_or("the daemon is not running")?;
                        client.telegram_reload()
                    });
                    let status = match reloaded {
                        Ok(_) => telegram_status(),
                        Err(error) => format!("Could not save: {error}"),
                    };
                    if let Some(frontend_tui::model::Overlay::Settings(s)) = &mut w.overlay {
                        s.telegram.status = status;
                    }
                    w.telegram = Some(self.telegram_page());
                    w.voice = Some(self.voice_page());
                    w.memory = Some(self.memory_page());
                }
                Outgoing::SaveMapLimit { limit } => {
                    let saved = config::load().and_then(|mut c| {
                        c.session.map_max_nodes = limit;
                        config::save(&c)
                    });
                    if let Err(error) = saved && let Some(frontend_tui::model::Overlay::Settings(s)) = &mut w.overlay {
                        s.memory.status = format!("Could not save: {error}");
                    }
                    w.memory = Some(self.memory_page());
                }
                Outgoing::SaveSearchPage { limit } => {
                    let saved = config::load().and_then(|mut c| {
                        c.session.search_max_linked = limit;
                        config::save(&c)
                    });
                    if let Err(error) = saved && let Some(frontend_tui::model::Overlay::Settings(s)) = &mut w.overlay {
                        s.memory.status = format!("Could not save: {error}");
                    }
                    w.memory = Some(self.memory_page());
                }
                Outgoing::DownloadVoiceModel { model } => {
                    let progress: std::sync::Arc<std::sync::Mutex<VoiceDownload>> = Default::default();
                    self.voice_download = Some(progress.clone());
                    let wanted = harness_speech::model(&model).ok_or_else(|| format!("no model {model}"));
                    let data = config::data_dir().map_err(|e| e.to_string());
                    std::thread::spawn(move || {
                        let ours = progress.clone();
                        let end = wanted.and_then(|wanted| {
                            harness_speech::install(&data?, wanted, &mut |step| {
                                let mb = |bytes: u64| bytes / (1024 * 1024);
                                ours.lock().expect("download").how_far = match step {
                                    harness_speech::Progress::Fetching { what, have, whole: Some(whole) } => format!("{what}: {} of {} MB", mb(have), mb(whole)),
                                    harness_speech::Progress::Fetching { what, have, whole: None } => format!("{what}: {} MB", mb(have)),
                                    harness_speech::Progress::Unpacking { what } => format!("{what}: unpacking"),
                                };
                            })
                        });
                        progress.lock().expect("download").end = Some(end);
                    });
                }
                Outgoing::ListVoiceDevices => {
                    let asked: std::sync::Arc<std::sync::Mutex<Option<harness_speech::InputDevices>>> = Default::default();
                    self.voice_devices = Some(asked.clone());
                    std::thread::spawn(move || *asked.lock().expect("devices") = Some(harness_speech::input_devices()));
                }
                Outgoing::SaveVoice { engine, model, device } => {
                    let saved = config::load().and_then(|mut c| {
                        c.voice.backend = engine;
                        c.voice.model = Some(model).filter(|m| !m.is_empty());
                        (c.voice.device, c.voice.device_name) = device.map_or((None, None), |d| (Some(d.id), Some(d.name)));
                        config::save(&c)
                    });
                    let reloaded = saved.map_err(|e| e.to_string()).and_then(|_| {
                        let client = config::data_dir().ok().and_then(|d| harness_daemon::client::Client::find(&d)).ok_or("the daemon is not running")?;
                        client.telegram_reload()
                    });
                    if let (Err(error), Some(frontend_tui::model::Overlay::Settings(s))) = (&reloaded, &mut w.overlay) {
                        s.voice.status = format!("Could not save: {error}");
                    }
                    w.voice = Some(self.voice_page());
                    w.memory = Some(self.memory_page());
                }
                Outgoing::StartVoiceTest { model, device } => {
                    let started = harness_speech::model(&model).ok_or_else(|| format!("no model {model}")).and_then(|model| Ok((model, harness_speech::Recording::start(device.as_deref())?)));
                    match started {
                        Ok((model, recording)) => self.voice_test = Some(VoiceTestRun { recording: Some(recording), model, heard: Default::default() }),
                        Err(why) => {
                            if let Some(frontend_tui::model::Overlay::Settings(s)) = &mut w.overlay {
                                s.voice.test = frontend_tui::model::VoiceTest::Idle;
                                s.voice.status = why;
                            }
                        }
                    }
                }
                Outgoing::StopVoiceTest => self.stop_voice_test(),
                Outgoing::SaveCallbacks { handlers, disabled } => {
                    let settings = harness_daemon::conditions::ProjectCallbacks { handlers: handlers.into_iter().collect(), disabled };
                    let note = match harness_daemon::conditions::write_project(&self.root, &settings) {
                        Ok(()) => format!("Saved .harness/callbacks.toml. {}", self.reload_daemon()),
                        Err(e) => format!("Could not save: {e}"),
                    };
                    w.callbacks = Some(self.callbacks_page());
                    w.telegram = Some(self.telegram_page());
                    w.voice = Some(self.voice_page());
                    w.memory = Some(self.memory_page());
                    w.view_mut().transcript.push(frontend_tui::model::Item::Tips { label: "CALLBACKS".into(), lines: vec![note] });
                }
                Outgoing::Reload => {
                    self.reload_by_user = true;
                    self.reload_asked.store(true, std::sync::atomic::Ordering::SeqCst);
                }
                Outgoing::ReloadCallbacks => {
                    let note = self.reload_daemon();
                    w.view_mut().transcript.push(frontend_tui::model::Item::Tips { label: "CALLBACKS".into(), lines: vec![note] });
                }
                Outgoing::SaveSettings { provider, model, context: chosen, delegate } => {
                    let provider = provider_id(&provider);
                    let auth = w.auth.clone().unwrap_or_default();
                    let context = auth.models.iter().chain(auth.provider_models.iter().flat_map(|(_, m)| m)).find(|m| m.id == model).and_then(|m| m.context);
                    let tab = w.tabs[w.active].id;
                    let now = self.sessions.choice_of(tab).map_or_else(|| (self.provider.clone(), self.model.clone()), |c| (c.provider, c.model));
                    let previous = Some(now).filter(|(p, m)| (p, m) != (&provider, &model));
                    let note = match self.save_settings_with(&provider, &model, context, chosen, Some(delegate)) {
                        Ok(()) => {
                            self.previous_adapter = previous;
                            self.apply_to_tab(w, tab, &provider, &model);
                            // The settings open next time as they were saved.
                            w.auth = Some(self.auth_page(w.auth.take()));
                            format!("This conversation and new ones use {provider} / {model}. Other conversations stay on theirs.")
                        }
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
                FromSession::Spawn { tab: master, nick, provider, model, task } => {
                    // A tab of the master's group, beside it; the user stays where they are.
                    let was = w.tabs[w.active].id;
                    frontend_tui::debug::open_tab(w, &nick);
                    let Some(mut opened) = w.tabs.pop() else { continue };
                    let id = opened.id;
                    opened.view.agent_name = Some(nick.clone());
                    opened.view.orchestra = Some("slave".to_string());
                    frontend_tui::live::show_mode(&mut opened.view);
                    opened.view.background.clear();
                    frontend_tui::live::set_status(&mut opened.view, "ADAPTER", &provider);
                    frontend_tui::live::set_status(&mut opened.view, "MODEL", &model);
                    let mut at = w.tabs.iter().position(|t| t.id == master).map_or(w.tabs.len(), |i| i + 1);
                    while w.tabs.get(at).is_some_and(|t| t.view.orchestra.as_deref() == Some("slave")) {
                        at += 1;
                    }
                    w.tabs.insert(at, opened);
                    w.active = w.tabs.iter().position(|t| t.id == was).unwrap_or(0);
                    if let Err(why) = self.sessions.adopt(id, master, &nick, &provider, &model, task) {
                        w.view_mut().transcript.push(frontend_tui::model::Item::Tips { label: "ORCHESTRA".into(), lines: vec![why] });
                    }
                }
                FromSession::CloseTab { tab } => {
                    if let Some(at) = w.tabs.iter().position(|t| t.id == tab)
                        && w.tabs.len() > 1
                    {
                        let was = w.tabs[w.active].id;
                        w.tabs.remove(at);
                        self.feeds.remove(&tab);
                        w.active = w.tabs.iter().position(|t| t.id == was).unwrap_or(at.min(w.tabs.len() - 1));
                    }
                }
                FromSession::Queued { tab, message } => {
                    if let Some(t) = w.tabs.iter_mut().find(|t| t.id == tab) {
                        t.view.queued.push(message);
                    }
                }
                FromSession::Entry { tab, entry } => {
                    if self.shown_up_to.get(&tab).is_some_and(|seq| entry.seq <= *seq) {
                        continue;
                    }
                    if self.context_chosen && matches!(entry.event, harness_api::Event::ContextWindow { .. }) {
                        continue;
                    }
                    if let harness_api::Event::StepDebug { on, .. } = entry.event {
                        self.note_step_debug(tab, on);
                    }
                    let feed = self.feeds.entry(tab).or_insert_with(|| Feed::new(local_time, window));
                    if let Some(t) = w.tabs.iter_mut().find(|t| t.id == tab) {
                        feed.apply(&mut t.view, &entry);
                        frontend_tui::live::name_tab(t);
                        // A plan is not billed per token: its COST says so, and what the same
                        // tokens would cost on the API.
                        if let harness_api::Event::TurnFinished { usage, .. } = &entry.event {
                            let field = |name: &str| t.view.status.fields.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone()).unwrap_or_default();
                            let (adapter, model, cost) = (field("ADAPTER"), field("MODEL"), field("COST"));
                            // What Claude Code itself reports; it may be covered by a plan.
                            if adapter == "claude-code" {
                                frontend_tui::live::set_status(&mut t.view, "COST", &format!("~{cost} by Claude Code"));
                            }
                            if adapter == "chatgpt" {
                                let total = self.plan_cost.entry(tab).or_default();
                                *total += harness_provider_openai::prices::cost_micro_usd(&model, usage).unwrap_or(0);
                                frontend_tui::live::set_status(&mut t.view, "COST", &format!("in plan (~${:.2} at API)", *total as f64 / 1e6));
                            }
                        }
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
                    if let Some(a) = w.tabs.iter_mut().find(|t| t.id == tab).and_then(|t| t.view.activity.as_mut()) {
                        a.state = "Waiting for your answer".to_string();
                    }
                    let view = permission(tab, &call, &capabilities, &self.root);
                    if w.overlay.is_none() {
                        w.overlay = Some(Overlay::Permission(view));
                    } else {
                        self.asks.push_back(view);
                    }
                }
                FromSession::Ended { tab, end } => {
                    if self.rebind_after.contains(&tab) && self.sessions.rebind(tab) {
                        self.rebind_after.remove(&tab);
                    }
                    let feed = self.feeds.entry(tab).or_insert_with(|| Feed::new(local_time, window));
                    if let Some(t) = w.tabs.iter_mut().find(|t| t.id == tab) {
                        feed.run_ended(&mut t.view);
                        if end == harness_core::RunEnd::Abandoned {
                            t.view.transcript.push(frontend_tui::model::Item::Tips { label: "STOPPED".into(), lines: vec!["Stopped. Say what to do instead.".to_string()] });
                        }
                    }
                }
                FromSession::Synced { tab, error } => {
                    self.syncing.remove(&tab);
                    if self.syncing.is_empty() && matches!(w.overlay, Some(Overlay::Syncing(_))) {
                        w.overlay = None;
                    }
                    if let Some(message) = error {
                        if let Some(t) = w.tabs.iter_mut().find(|t| t.id == tab) {
                            t.view.transcript.push(Item::Tips { label: "COULD NOT SYNC THE CONVERSATION".into(), lines: message.lines().map(str::to_string).collect() });
                        }
                        // CON-81: it stays on the adapter it had.
                        if let Some((provider, model)) = self.previous_adapter.take()
                            && self.save_settings(&provider, &model, None, None).is_ok()
                        {
                            self.apply_to_tab(w, tab, &provider, &model);
                        }
                    } else if self.syncing.is_empty() {
                        self.previous_adapter = None;
                    }
                }
                FromSession::Unavailable { tab, message } => {
                    self.syncing.remove(&tab);
                    if self.syncing.is_empty() && matches!(w.overlay, Some(Overlay::Syncing(_))) {
                        w.overlay = None;
                    }
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
/// A provider row of the settings as the configuration names it.
fn provider_id(row: &str) -> String {
    match row {
        "Anthropic API" => "anthropic".to_string(),
        "OpenAI-compatible" => "openai".to_string(),
        "Local" => "local".to_string(),
        frontend_tui::model::PLAN_PROVIDER => "chatgpt".to_string(),
        frontend_tui::model::DELEGATE_PROVIDER => "claude-code".to_string(),
        other => other.to_string(),
    }
}

fn harness_session_store(data_dir: &std::path::Path) -> harness_auth_chatgpt::Store {
    harness_auth_chatgpt::Store::new(data_dir)
}

fn plan_base_url(session: &harness_session::Settings) -> String {
    provider_base_url(session, "chatgpt")
}

/// Where a provider's server is: as configured, or its usual place.
fn provider_base_url(session: &harness_session::Settings, id: &str) -> String {
    let usual = if id == "local" { LOCAL_BASE_URL } else { "https://api.openai.com/v1" };
    session.providers.get(id).and_then(|p| p["base_url"].as_str()).unwrap_or(usual).to_string()
}

/// Ollama's address, the usual local server.
const LOCAL_BASE_URL: &str = "http://127.0.0.1:11434/v1";

/// Opens the user's browser. MULTITOOL_BROWSER names another command, or `none`.
fn open_browser(url: &str) -> bool {
    use std::process::{Command, Stdio};
    let mut command = match std::env::var("MULTITOOL_BROWSER") {
        Ok(name) if name == "none" => return false,
        Ok(name) => Command::new(name),
        Err(_) if cfg!(target_os = "macos") => Command::new("open"),
        Err(_) if cfg!(windows) => {
            let mut c = Command::new("cmd");
            c.args(["/C", "start", ""]);
            c
        }
        Err(_) => Command::new("xdg-open"),
    };
    command.arg(url).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().is_ok()
}

/// What a provider's models cost and hold, as lines for the dropdown. A plan's tokens are not
/// charged, so its line says so, with the API's price for comparison.
fn choices(row: &str, models: Vec<harness_auth_chatgpt::ListedModel>) -> Vec<frontend_tui::model::ModelChoice> {
    let money = |(input, output): (f64, f64)| format!("${input:.2} in / ${output:.2} out per 1M");
    models
        .into_iter()
        .map(|m| {
            let known = m.per_million.or_else(|| if row == "Anthropic API" { harness_provider_anthropic::per_million(&m.id) } else { harness_provider_openai::prices::per_million(&m.id) });
            let cost = match (row, known) {
                ("Local", _) => "local, free".to_string(),
                (frontend_tui::model::PLAN_PROVIDER, Some(price)) => format!("in your plan (API {})", money(price)),
                (frontend_tui::model::PLAN_PROVIDER, None) => "in your plan".to_string(),
                (_, Some(price)) => money(price),
                (_, None) => "price unknown".to_string(),
            };
            let context = m.context.map_or(String::new(), |c| format!(" · {}k", c / 1000));
            frontend_tui::model::ModelChoice { id: m.id, detail: format!("{cost}{context}"), context: m.context }
        })
        .collect()
}

fn only_chatgpt(provider: &str) -> Result<()> {
    match provider {
        "chatgpt" => Ok(()),
        "claude" | "anthropic" => bail!("Anthropic does not permit Claude subscription sign-in in other apps; set ANTHROPIC_API_KEY with a key from Claude Console"),
        other => bail!("no sign-in for {other:?}: only chatgpt"),
    }
}

/// `multitool login chatgpt`: the user signs in on OpenAI's page; nothing here sees a password.
fn login(provider: &str) -> Result<()> {
    only_chatgpt(provider)?;
    let store = harness_auth_chatgpt::Store::new(&config::data_dir()?);
    let session = config::load()?.session;
    let issuer = session.auth.get("chatgpt").and_then(|a| a["issuer"].as_str()).unwrap_or(harness_auth_chatgpt::ISSUER).to_string();
    let base = session.providers.get("chatgpt").and_then(|p| p["base_url"].as_str()).unwrap_or("https://api.openai.com/v1").to_string();
    let pending = harness_auth_chatgpt::begin_login(&store, &issuer, 1455).map_err(|e| anyhow!(e))?;
    println!("Open this in your browser and sign in:\n\n{}\n\nWaiting for it (up to 10 minutes)...", pending.url);
    let credential = pending.finish(&store, std::time::Duration::from_secs(600)).map_err(|e| anyhow!(e))?;
    println!("Signed in{}.", credential.email.map_or(String::new(), |e| format!(" as {e}")));
    match harness_auth_chatgpt::list_models(&base, &credential.access_token) {
        Ok(models) => {
            println!("Models this account can use:");
            models.iter().for_each(|m| println!("  {}  {}", m.id, m.name));
        }
        Err(e) => println!("Could not list the models: {e}"),
    }
    println!("\nIn {}:\n\n[session]\nprovider = \"chatgpt\"\nmodel = \"<one of the above>\"\n\n[session.providers.chatgpt]\nkind = \"openai-responses\"\n\n[session.auth.chatgpt]\noauth = \"chatgpt\"", config::path()?.display());
    Ok(())
}

fn init_project(root: &str) -> Result<Vec<String>, String> {
    pollster::block_on(harness_docs_mirror::project_init(&harness_platform_native::NativeFs, root)).map_err(|e| e.to_string())
}

/// `multitool docs check`: exit 1 on a finding. With no docs tree there is nothing to compare.
fn docs_check() -> Result<()> {
    let root = std::env::current_dir()?.to_string_lossy().into_owned();
    let fs = harness_platform_native::NativeFs;
    let Some(tree) = pollster::block_on(harness_docs_mirror::Tree::load(&fs, &root)) else {
        println!("no docs tree here: multitool init creates one");
        return Ok(());
    };
    let mut found = tree.check_files();
    let memory = harness_session::project_memory(&config::data_dir()?.to_string_lossy(), &root).map_err(|e| anyhow!(e))?;
    found.extend(pollster::block_on(tree.check_journal(&memory)).map_err(|e| anyhow!(e.to_string()))?);
    found.iter().for_each(|f| println!("{}", f.0));
    if found.is_empty() {
        println!("the documentation and the journal agree");
        return Ok(());
    }
    bail!("{} findings", found.len())
}

fn run_once(prompt: &str, yes: bool, plan: bool, step_debug: bool, task: Option<Vec<String>>) -> Result<()> {
    let from_stdin = if prompt == "-" { Some(std::io::read_to_string(std::io::stdin()).context("reading the prompt from stdin")?) } else { None };
    let prompt = from_stdin.as_deref().unwrap_or(prompt);
    use harness_session::{FromSession, Sessions, real_factory};
    let root = std::env::current_dir()?.to_string_lossy().into_owned();
    let data = config::data_dir()?;
    let settings = config::load()?.session;
    // Step-debug needs what a window has: a daemon, a callback URL, and the daemon told the mode.
    let window = if step_debug {
        let client = harness_daemon::client::Client::find_or_start(&data, &harness_daemon::own_program()?).map_err(|e| anyhow!(e))?;
        let registered = client.register(&root).map_err(|e| anyhow!(e))?;
        client.set_step_debug(&registered.window, true).map_err(|e| anyhow!(e))?;
        println!("window      {} on the daemon; callbacks go to {}", registered.window, registered.url);
        Some((client, registered))
    } else {
        None
    };
    let factory = match &window {
        Some((_, registered)) => {
            let urls: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<u64, String>>> = Default::default();
            if let Ok(mut known) = urls.lock() {
                known.insert(1, registered.url.clone());
            }
            let note = std::sync::Arc::new(move |tab: u64| callback_note(&urls, tab));
            let summary = selection_summary(&settings);
            harness_session::with_host(settings, root.clone(), data.clone(), host_for(note, Default::default(), summary))
        }
        None => real_factory(settings, root.clone(), data.clone()),
    };
    let mut sessions = Sessions::new(factory, root).with_data_dir(&data.to_string_lossy());
    if plan {
        sessions.plan(1, true);
    }
    if step_debug {
        sessions.step_debug(1, true);
    }
    match task {
        Some(steps) => {
            let mut answer = sessions.task(1, prompt.to_string(), steps.clone());
            if let (true, Err(harness_session::TaskRefused::NotApproved { commands, text })) = (yes, &answer) {
                println!("approved    the project's checks: {}", commands.join("; "));
                sessions.approve_acceptance(text).map_err(|e| anyhow!(e))?;
                answer = sessions.task(1, prompt.to_string(), steps);
            }
            answer.map_err(|e| anyhow!("{e}; --yes approves them"))?;
        }
        None => sessions.prompt(1, prompt.to_string()),
    }
    loop {
        let Some(message) = sessions.next(std::time::Duration::from_secs(3600)) else { bail!("no event for an hour") };
        match message {
            FromSession::Entry { entry, .. } => {
                println!("{}", harness_scenarios::line(&entry));
                if let (harness_api::Event::StepDebug { on, .. }, Some((client, registered))) = (&entry.event, &window) {
                    let _ = client.set_step_debug(&registered.window, *on);
                }
            }
            FromSession::Ask { tab, call, .. } => {
                println!("ask         {} {} -> {}", call.name, call.input, if yes { "yes" } else { "no" });
                sessions.answer(tab, if yes { harness_api::Answer::AllowOnce } else { harness_api::Answer::Deny });
            }
            FromSession::Ended { end, .. } => {
                println!("ended: {end:?}");
                return Ok(());
            }
            FromSession::Synced { .. } => {}
            FromSession::Unavailable { message, .. } => bail!("{message}"),
            FromSession::Spawn { .. } | FromSession::CloseTab { .. } | FromSession::Queued { .. } => {}
        }
    }
}

/// `multitool import`: lists a source's conversations, or brings one in as a session here.
fn import_conversation(source: &str, id: Option<&str>) -> Result<()> {
    let root = std::env::current_dir()?.to_string_lossy().into_owned();
    let conversations = harness_session::conversation_source(source).map_err(|e| anyhow!(e))?;
    let Some(id) = id else {
        let found = conversations.list(&root).map_err(|e| anyhow!(e.to_string()))?;
        if found.is_empty() {
            println!("{source} has no conversations for this directory");
        }
        for c in found {
            println!("{}  {:>8.1} MB  {}", c.id, c.bytes as f64 / 1e6, c.title);
        }
        return Ok(());
    };
    let data_dir = config::data_dir()?;
    // One conversation, wherever it is opened: it is linked once, and an open catches up.
    if let Some(session) = harness_session::linked_session(&data_dir.to_string_lossy(), &root, source, id) {
        println!("already here, linked: session {session}");
        println!("Opening it brings in what {source} gained since. Open it from the start page, in this directory.");
        return Ok(());
    }
    let imported = conversations.read(&root, id).map_err(|e| anyhow!(e.to_string()))?;
    let now_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_millis() as u64;
    let session = harness_session::store_imported(&data_dir.to_string_lossy(), &root, &imported, now_ms).map_err(|e| anyhow!(e))?;
    println!("imported \"{}\": {} entries, {} damaged lines passed over", imported.info.title, imported.entries.len(), imported.damaged);
    println!("session: {session}");
    println!("It is linked, not copied: one conversation in {source} and here, under the same id. Only one of them can write at a time.");
    println!("Open it from the start page, in this directory.");
    Ok(())
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
        self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton,
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
    execute!(std::io::stdout(), EnableMouseCapture, EnableBracketedPaste)?;
    // ratatui's own hook gives the terminal back, but knows nothing of these: after a panic the
    // shell would be typed mouse reports at (journal#595).
    let restores_terminal = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // A panic while drawing is caught, and the window goes on: the terminal stays as it is,
        // and its words go to a file, for they would spoil the screen (CON-113).
        if frontend_tui::transcript::in_guarded_draw() {
            if let Ok(dir) = config::data_dir()
                && let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("draw-errors.log"))
            {
                let _ = std::io::Write::write_fmt(&mut file, format_args!("{} {info}\n", chrono::Utc::now().to_rfc3339()));
            }
            return;
        }
        let _ = execute!(std::io::stdout(), DisableMouseCapture, DisableBracketedPaste);
        restores_terminal(info);
    }));
    let mut ctrl_c_at = None;
    // Text that came as a paste and has no composer to go to yet (the start page, a prompt on
    // screen), with whether its Enter came too. An Enter right after a paste belongs to the paste:
    // it must not open a session or answer a prompt.
    let (mut held_paste, mut held_enter, mut pasted_at) = (String::new(), false, None::<std::time::Instant>);
    let mut inbound_at = None::<std::time::Instant>;
    // The settings were opened from the start page: closing them goes back there, the new choice shown.
    let mut settings_from_start = false;
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
            live.tick_linking(&mut state);
            dirty |= live.receive(&mut state);
            dirty |= live.receive_daemon(&mut state);
            dirty |= live.tick_reload(&mut state, &current.id)?;
            if let Some(path) = live.sessions.screenshot_asked() {
                let size = terminal.size()?;
                // A failure here is the debug call's to report, by its timeout: the window goes on.
                let _ = draw_to_file(&state, &current, size.width, size.height, std::path::Path::new(&path));
            }
            if let frontend_tui::model::Mode::Session(w) = &mut state.mode {
                // CON-79, CON-94: a conversation's adapter stays as it is while that conversation runs.
                let run_on = w.tabs[w.active].view.generating || matches!(w.overlay, Some(frontend_tui::model::Overlay::Permission(_))) || !live.asks.is_empty();
                if let Some(frontend_tui::model::Overlay::Settings(s)) = &mut w.overlay
                    && s.run_on != run_on
                {
                    s.run_on = run_on;
                    if run_on {
                        s.model_menu = None;
                        s.editing_model = false;
                    }
                    dirty = true;
                }
                // The moon goes round; everything else waits for the daemon (CON-81).
                if let Some(frontend_tui::model::Overlay::Syncing(s)) = &mut w.overlay {
                    s.now_ms = chrono::Utc::now().timestamp_millis() as u64;
                    dirty = true;
                }
                // Messages go to the tab in front, once there is one; a run on takes them after it.
                let waiting_for_daemon = matches!(w.overlay, Some(frontend_tui::model::Overlay::Syncing(_)));
                for (label, body) in if waiting_for_daemon { Vec::new() } else { std::mem::take(&mut live.held_inbound) } {
                    let (kind, from) = frontend_tui::live::inbound_kind(&label);
                    w.toast = Some(frontend_tui::model::Toast { kind, from, text: body.clone() });
                    let tab = w.tabs[w.active].id;
                    w.tabs[w.active].view.generating = true;
                    live.sessions.inbound(tab, label, body);
                    dirty = true;
                }
                // What a paste left waiting has its composer now.
                if !held_paste.is_empty() && w.overlay.is_none() {
                    w.view_mut().composer.insert(&std::mem::take(&mut held_paste));
                    dirty = true;
                    if std::mem::take(&mut held_enter) {
                        input::key(&mut state, Key::Enter);
                        live.send(&mut state);
                    }
                }
            }
            if let frontend_tui::model::Mode::Session(w) = &mut state.mode {
                let now = chrono::Utc::now().timestamp_millis() as u64;
                dirty |= w.tabs.iter_mut().fold(false, |any, t| frontend_tui::live::tick_pulses(&mut t.view, now) | any);
                dirty |= w.tabs.iter_mut().fold(false, |any, t| frontend_tui::live::tick_activity(&mut t.view, now) | any);
                dirty |= live.tick_prompt(w);
                live.tick_first_start(w);
                dirty |= live.tick_toast(w);
                dirty |= live.tick_login(w);
                w.new_tab_runs_on = Some((live.provider.clone(), live.model.clone()));
                dirty |= live.tick_voice_download(w);
                dirty |= live.tick_voice_test(w);
                dirty |= live.tick_voice_devices(w);
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
                Event::Paste(text) => {
                    pasted_at = Some(std::time::Instant::now());
                    // A message a launcher typed in is that message, with its own card: not the
                    // user's words, and its Enter is nobody's.
                    if let Some((label, body)) = frontend_tui::live::parse_inbound(&text) {
                        inbound_at = pasted_at;
                        live.held_inbound.push((label, body));
                        continue;
                    }
                    match &mut state.mode {
                        frontend_tui::model::Mode::Session(w) if w.overlay.is_none() => w.view_mut().composer.insert(&text),
                        // A field being typed in takes it, line breaks aside.
                        frontend_tui::model::Mode::Session(w) if matches!(&w.overlay, Some(frontend_tui::model::Overlay::Settings(s)) if s.editing_model || s.telegram.editing || s.memory.editing || s.auth.entry.is_some()) => {
                            pasted_at = None;
                            for c in text.chars().filter(|c| !c.is_control()) {
                                input::key(&mut state, Key::Char(c));
                            }
                        }
                        _ => held_paste.push_str(&text),
                    }
                    continue;
                }
                Event::Key(k) if k.kind == KeyEventKind::Press && k.code == KeyCode::Enter && inbound_at.take().is_some_and(|at| at.elapsed() < std::time::Duration::from_secs(1)) => continue,
                Event::Key(k) if k.kind == KeyEventKind::Press && k.code == KeyCode::Enter && !held_paste.is_empty() && pasted_at.is_some_and(|at| at.elapsed() < std::time::Duration::from_secs(1)) => {
                    held_enter = true;
                    continue;
                }
                Event::Key(k) if k.kind == KeyEventKind::Press => {
                    if k.code == KeyCode::Char('c') && k.modifiers.contains(KeyModifiers::CONTROL) {
                        if frontend_tui::input::interrupt(&mut state, &mut ctrl_c_at, std::time::Instant::now()) == frontend_tui::input::Outcome::Quit {
                            return Ok(());
                        }
                        live.send(&mut state);
                        dirty = true;
                        continue;
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
                        KeyCode::Delete => Key::Delete,
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
                            settings_from_start = true;
                        }
                        // Settings, tour and help screens come in later stages.
                        _ => {}
                    }
                }
                Outcome::Stay | Outcome::OpenSettings => {}
            }
            live.send(&mut state);
            if settings_from_start && matches!(&state.mode, Mode::Session(w) if w.overlay.is_none()) {
                settings_from_start = false;
                state = live.startup();
            }
        }
    })();
    let _ = execute!(std::io::stdout(), DisableMouseCapture, DisableBracketedPaste);
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
