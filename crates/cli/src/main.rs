//! `multitool`: the harness binary. The terminal UI runs real sessions (harness-session); the
//! debug commands `screen` and `trace` show a named UI state or a loop scenario on their own.

use std::path::PathBuf;

use anyhow::{Context, Result, anyhow, bail};
use clap::{Parser, Subcommand};
use frontend_tui::{Theme, fixtures};
use tui_snapshot::{Raster, side_by_side};

mod config;
mod file_picker;
mod plugins;
mod reload;

#[derive(Parser)]
#[command(name = "multitool", version, about = "harness-multitool")]
struct Cli {
    /// The profile this run works on, for this run only, as autostart wants it (CON-127). Without it,
    /// the one last chosen with Use in the settings, else `default`.
    #[arg(long, global = true)]
    profile: Option<String>,
    /// Work in this folder, as if started there: a terminal opened from Telegram names it (CON-167).
    #[arg(long, global = true)]
    dir: Option<PathBuf>,
    /// Open this conversation in a tab at once, with no start page (CON-167).
    #[arg(long, conflicts_with = "new")]
    resume: Option<String>,
    /// Open a new session tab at once, with no start page (CON-167).
    #[arg(long)]
    new: bool,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(clap::Subcommand)]
enum DaemonAction {
    /// Stops the running daemon, its runs included, and starts this build's in its place, on the
    /// same port when it is free; open windows reopen their conversations in it (CON-128).
    Restart,
    /// Stops the running daemon, its runs included, and starts none (CON-128).
    Stop {
        /// Only if the running daemon is this process: the service's ExecStop names its own, so
        /// that stopping the service never stops a daemon it does not run (journal#1193).
        #[arg(long)]
        pid: Option<u32>,
    },
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
    /// Write a conversation of this directory from one provider's form into another's, as the
    /// daemon does when it is moved (CON-134), and print the new id. The original is only read.
    Convert {
        /// `claude-code` or `codex`
        from: String,
        /// `claude-code` or `codex`
        to: String,
        id: String,
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
enum ProfileAction {
    /// Each profile, its adapter and model, and whether its Claude Code and ChatGPT are signed in.
    List,
    /// A new profile, entirely clean: built-in defaults, no key, no sign-in.
    Add { name: String },
}

#[derive(Subcommand)]
enum ChatAction {
    /// Pack a conversation of this project, with its Claude Code form and the project's journal,
    /// mind maps and flowcharts, into a .zip in this folder; with none named, the newest. An
    /// existing file is never written over (CON-171).
    Dump {
        /// The conversation: its id, or its name as the start page shows it.
        conversation: Option<String>,
        /// Where the archive goes, instead of `multitool_dumps/<name>-<id>-<time>.zip` here.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Bring a conversation in from such an archive, into this folder's project: under new ids
    /// (CON-175), its project paths moved here, the journal once (CON-171).
    Load {
        archive: PathBuf,
        /// List it under another name.
        #[arg(long)]
        name: Option<String>,
    },
    /// This project's conversations, as the Chats tab lists them.
    List,
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
    /// The project's todos as issues in its tracker (ADR 0028): status, on, off, connect, gh-dir, tracker.
    Issues {
        #[command(subcommand)]
        action: IssuesAction,
    },
}

#[derive(Subcommand)]
enum IssuesAction {
    /// The settings, the connection, the place, and why nothing is sent if it is not.
    Status,
    /// Send this project's todos to issues: the open ones now, the rest as they come.
    On {
        /// The repository is public, and its issues may be too.
        #[arg(long)]
        public: bool,
    },
    /// Send nothing more; what was queued is dropped.
    Off,
    /// Sign gh in, by its own `gh auth login`, in the project's gh folder.
    Connect,
    /// Run gh with this configuration folder, one you already have (an account's); `-` for the project's own.
    GhDir { path: String },
    /// The tracker: github, gitlab, asana or clickup (only github is built).
    Tracker { name: String },
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
    /// Sign this profile in: `chatgpt` on OpenAI's own page, or `claude-code`, which hands the terminal
    /// to the unmodified `claude auth login` in the profile's own Claude Code directory (CON-123). The
    /// harness never sees a Claude token.
    Login {
        /// `chatgpt`, `claude-code` or `codex`
        provider: String,
    },
    /// Sign this profile out: `chatgpt` revokes and clears the token; `claude-code` runs
    /// `claude auth logout` in the profile's Claude Code directory.
    Logout {
        /// `chatgpt`, `claude-code` or `codex`
        provider: String,
    },
    /// The profiles: `list` them, or `add <name>` a new, clean one (CON-120, CON-125).
    Profile {
        #[command(subcommand)]
        action: ProfileAction,
    },
    /// Bring in a conversation another agent keeps for this directory; with no id, list them.
    /// The original is only read, and goes on in a fork (ADR 0022).
    Import {
        /// `claude-code`
        source: String,
        id: Option<String>,
    },
    /// A conversation dumped to a .zip and loaded back, here or on another machine (CON-171).
    Chat {
        #[command(subcommand)]
        action: ChatAction,
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
    /// The address of the daemon's dashboard (ADR 0029): agents online and games between them.
    Dashboard,
    /// The daemon: the single entry point of callbacks (the terminal UI starts it when needed).
    Daemon {
        /// The port on localhost; by default one is picked.
        #[arg(long, default_value_t = 0)]
        port: u16,
        /// Don't start one: print what the running daemon counts.
        #[arg(long)]
        status: bool,
        #[command(subcommand)]
        action: Option<DaemonAction>,
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
    let cli = Cli::parse();
    if let Some(dir) = &cli.dir {
        std::env::set_current_dir(dir).with_context(|| format!("--dir {}", dir.display()))?;
    }
    // Every start: the profiles are made or moved in first (CON-120), then this run's is chosen.
    // --profile for this run only (autostart); else the one last chosen in the settings (CON-127).
    let profile = match cli.profile {
        Some(name) => name,
        None => config::starting()?,
    };
    // The user's own folders, for every multitool started from this one, before this profile's
    // are set (journal#1025).
    for (marker, value) in config::own_markers() {
        // SAFETY: first thing in main, before any thread is started.
        unsafe { std::env::set_var(marker, value) };
    }
    config::select(&profile)?;
    for (name, dir) in config::sign_in_environment()? {
        // SAFETY: first thing in main, before any thread is started. This process's environment is
        // what a window sends its daemon, so its sessions reach this profile's sign-ins (CON-122).
        unsafe { std::env::set_var(name, dir) };
    }
    // Bare `multitool` opens the start page, as run.sh does (journal#780).
    // --resume and --new: one tab at once, as a reload hands its tabs over (CON-167).
    let opened = match (&cli.resume, cli.new) {
        (Some(session), _) => Some(Some(session.clone())),
        (None, true) => Some(None),
        (None, false) => None,
    };
    let command = match (cli.command, opened) {
        (Some(command), _) => command,
        (None, Some(session)) => Command::Preview { name: "splash".into(), theme: "dark".into(), script: None, handoff: Some(one_tab_handoff(session)?) },
        (None, None) => Command::Preview { name: "splash".into(), theme: "dark".into(), script: None, handoff: None },
    };
    match command {
        Command::Preview { name, theme, script, handoff } => {
            // A window: it never hangs on the daemon (CON-132).
            harness_daemon::client::this_is_a_window();
            let shown = preview(&name, &theme, script.as_deref(), handoff.as_deref());
            // A window that ran its sessions itself (Local) ends their delegates too (journal#1021).
            harness_platform_native::children::end_all(std::time::Duration::from_secs(3));
            shown
        }
        Command::Trace { name, json } => trace(name.as_deref(), json),
        Command::Login { provider } if provider == "claude-code" => claude_code_auth("login"),
        Command::Login { provider } if provider == "codex" => codex_auth(&config::current(), true),
        Command::Logout { provider } if provider == "codex" => codex_auth(&config::current(), false),
        Command::Login { provider } => login(&provider),
        Command::Logout { provider } if provider == "claude-code" => claude_code_auth("logout"),
        Command::Profile { action: ProfileAction::Add { name } } => {
            let dir = config::create(&name)?;
            println!("Made the profile {name} in {}. Start a window on it with: multitool --profile {name}", dir.display());
            println!("Choose its adapter in the settings there; sign Claude Code in with: multitool --profile {name} login claude-code");
            Ok(())
        }
        Command::Profile { action: ProfileAction::List } => list_profiles(),
        Command::Logout { provider } => {
            only_chatgpt(&provider)?;
            let store = harness_session_store(&config::data_dir()?);
            let confirmed = harness_auth_chatgpt::logout(&store).map_err(|e| anyhow!(e))?;
            println!("{}", if confirmed { "signed out" } else { "signed out here, but OpenAI did not confirm the revocation: disconnect the app in ChatGPT's settings" });
            Ok(())
        }
        Command::Import { source, id } => import_conversation(&source, id.as_deref()),
        Command::Dashboard => {
            let data = config::data_dir()?;
            let text = std::fs::read_to_string(harness_daemon::address_path(&data)).map_err(|_| anyhow!("no daemon is running: start multitool, or `multitool daemon`"))?;
            let address: harness_daemon::Address = serde_json::from_str(&text)?;
            println!("http://127.0.0.1:{}/dashboard?token={}", address.port, address.admin);
            println!("Open it in a browser on this machine. The token lets whoever has it see and start games: keep it to yourself.");
            Ok(())
        }
        Command::Chat { action } => chat(action),
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
                JournalAction::Issues { action } => journal_issues(&data, &root, action)?,
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
        Command::Debug { action: DebugAction::Convert { from, to, id } } => {
            let root = std::env::current_dir()?.to_string_lossy().into_owned();
            // How far it got, on stderr, as a window's toast shows it (CON-144).
            harness_api::platform::progress::bind(harness_session::throttled(|p| {
                let unit = if p.unit == harness_api::platform::progress::Unit::Bytes { "bytes" } else { "messages" };
                eprintln!("{}: {} / {} {unit}", p.what, p.done, p.total);
            }));
            let source = harness_session::conversation_source(&from).map_err(|e| anyhow!(e))?;
            let target = harness_session::conversation_source(&to).map_err(|e| anyhow!(e))?;
            let read = source.read(&root, &id).map_err(|e| anyhow!(e.to_string()))?;
            let state = harness_api::SessionState::fold(read.entries.iter().map(|e| &e.event));
            let history = harness_api::provider_ready(&state.messages[state.compacted_upto.min(state.messages.len())..]);
            let written = target.write(&root, &read.info.title, &history).map_err(|e| anyhow!(e.to_string()))?;
            println!("{} messages of {from} {id} written as {to} {written}", history.len());
            Ok(())
        }
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
        Command::Daemon { action: Some(DaemonAction::Restart), .. } => daemon_restart(true, None),
        Command::Daemon { action: Some(DaemonAction::Stop { pid }), .. } => daemon_restart(false, pid),
        Command::Daemon { status: true, .. } => {
            let client = harness_daemon::client::Client::find(&config::data_dir()?).ok_or_else(|| anyhow!("no daemon is running"))?;
            println!("{}", client.stats().map_err(|e| anyhow!(e))?);
            Ok(())
        }
        Command::Daemon { port, .. } => {
            let dir = config::data_dir()?;
            // The port it had before if it is free: callback URLs stay as they were.
            let wanted = if port == 0 { harness_daemon::last_port(&dir).unwrap_or(0) } else { port };
            // A daemon that just stopped can hold its port a while: up to CLOSING (5 s) for its runs and
            // 3 s for its delegates to end (journal#1021). Wait for it rather than drift to another port,
            // which would leave every open dashboard and callback URL pointing nowhere (journal#1054).
            let mut started = harness_daemon::Daemon::start_with(&dir, wanted, plugins::callback_handlers());
            let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while started.is_err() && wanted != 0 && std::time::Instant::now() < until {
                std::thread::sleep(std::time::Duration::from_millis(100));
                started = harness_daemon::Daemon::start_with(&dir, wanted, plugins::callback_handlers());
            }
            let daemon = started.or_else(|_| harness_daemon::Daemon::start_with(&dir, port, plugins::callback_handlers())).map_err(|e| anyhow!(e))?;
            daemon.telegram_from(std::sync::Arc::new(config::telegram));
            // The conversations run here, whichever window asks (ADR 0024).
            let make_host: harness_session::remote::MakeHost = std::sync::Arc::new(|settings, note, reload| host_for(note, reload, selection_summary(settings)));
            daemon.host_conversations(harness_session::remote::DaemonHost::new(&dir, make_host));
            println!("daemon on 127.0.0.1:{}", daemon.port());
            daemon.wait_for_shutdown();
            // A run still going holds its delegate, and its thread is not unwound at exit: end
            // them all, so none outlives this daemon (journal#1021).
            harness_platform_native::children::end_all(std::time::Duration::from_secs(3));
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
/// What the thread bringing in a Claude Code conversation says: how far it got, then the end.
enum Linking {
    Progress(harness_api::platform::progress::Progress),
    Done(Result<harness_session::SessionInfo, String>),
}

/// A tab's conversation starts being moved or brought in line: its toast goes up (CON-144).
fn start_moving(w: &mut frontend_tui::model::Workspace, tab: u64, name: &str, what: &str, to: &str) {
    w.moving.retain(|m| m.tab != tab);
    let now_ms = chrono::Utc::now().timestamp_millis() as u64;
    w.moving.push(frontend_tui::model::Moving { tab, name: name.to_string(), what: what.to_string(), to: to.to_string(), now_ms, ..Default::default() });
}

/// What a send into a conversation being moved is told (CON-144).
fn moving_refusal(m: &frontend_tui::model::Moving) -> String {
    if m.to.is_empty() {
        "Not sent: this conversation is being brought in line. Send it again when its progress toast is gone.".to_string()
    } else {
        format!("Not sent: this conversation is being moved to {}. Send it again when its progress toast is gone.", m.to)
    }
}

/// An adapter's name for the user.
fn provider_label(provider: &str) -> String {
    match provider {
        "claude-code" => "Claude Code".to_string(),
        "codex" => "Codex".to_string(),
        other => other.to_string(),
    }
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
        // Settings > Behaviour, and what an exit does without asking (CON-181).
        let behaviour = behaviour_now();
        w.on_exit = behaviour.exit().0.map(|leave| leave.as_str().to_string());
        w.behaviour = Some(behaviour_page(&behaviour, String::new()));
        let profile = profile_label();
        w.view_mut().status.fields = [("MODE", "normal"), ("PROFILE", profile.as_str()), ("ADAPTER", provider), ("MODEL", model), ("TOKENS", "0"), ("COST", "$0.00"), ("BUDGET", budget)]
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .to_vec();
    }
    state
}

/// Settings > Behaviour as the configuration has it (CON-181).
fn behaviour_now() -> config::Behaviour {
    config::load().map(|c| c.behaviour).unwrap_or_default()
}

fn behaviour_page(b: &config::Behaviour, status: String) -> frontend_tui::model::BehaviourPage {
    let warnings: Vec<String> = [b.exit().1, b.loss().1].into_iter().flatten().collect();
    let status = if status.is_empty() { warnings.join("; ") } else { status };
    frontend_tui::model::BehaviourPage { cursor: 0, on_exit: b.exit().0.map(|l| l.as_str().to_string()), on_loss: b.loss().0.as_str().to_string(), status }
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
    /// The Journal page as a check of gh found it, off the window's thread (CON-132).
    journal_checks: (std::sync::mpsc::Sender<frontend_tui::model::JournalPage>, std::sync::mpsc::Receiver<frontend_tui::model::JournalPage>),
    /// What a Chats tab's dump or load came to (CON-171).
    chat_jobs: (std::sync::mpsc::Sender<ChatJob>, std::sync::mpsc::Receiver<ChatJob>),
    /// The number the next dump or load of the Chats tab gets, for its toast (CON-172).
    chat_job_next: u64,
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
    /// A profile's Claude Code sign-in or sign-out, waiting for the loop to hand it the terminal.
    terminal_wanted: Option<(String, ClaudeStep)>,
    /// The environment of the profile this window moved onto, for the sessions it runs itself;
    /// None while it is on the one it started on, whose sign-ins are this process's own (journal#824).
    local_env: Option<harness_session::WindowEnv>,
    /// The reload waiting for its safe point builds nothing: the window restarts on the installed
    /// binary (CON-129).
    installed_swap: bool,
    /// Each loss and each connection made of the window's connection to the daemon (CON-131).
    connection: Option<std::sync::mpsc::Receiver<harness_daemon::client::Connection>>,
    voice_test: Option<VoiceTestRun>,
    voice_devices: Option<std::sync::Arc<std::sync::Mutex<Option<harness_speech::InputDevices>>>>,
    /// The agent's `plugins_reload` tool raises this.
    reload_asked: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Tabs whose conversation the daemon is bringing in line: the window waits (CON-81).
    syncing: std::collections::HashSet<u64>,
    /// What to go back to if the change of adapter under way fails.
    previous_adapter: Option<(String, String)>,
    /// A conversation of Claude Code's being linked, off the window's thread.
    linking: Option<std::sync::mpsc::Receiver<Linking>>,
    /// The reload in hand was the user's own.
    reload_by_user: bool,
    /// The tab to tell how the reload in hand went.
    reload_for: Option<u64>,
    /// A rebuild in progress.
    reloading: Option<std::sync::mpsc::Receiver<Result<reload::Built, String>>>,
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
            if self.windows.contains(&tab) {
                continue;
            }
            // This window's own connection, never waited for here (CON-132): a tab not yet
            // registered is tried again on a later turn, once the connection is there.
            let client = match self.sessions.client() {
                Some(client) => client,
                None => {
                    let (Ok(dir), Ok(program)) = (config::data_dir(), harness_daemon::own_program()) else { continue };
                    harness_daemon::client::Client::for_window(&dir, &program)
                }
            };
            if !client.current() {
                continue;
            }
            let Ok(window) = client.register(&self.root) else { continue };
            self.windows.insert(tab);
            self.listen(tab, client, window);
        }
        // The daemon may have just started: the settings should say so.
        if self.windows.len() != before {
            w.callbacks = Some(self.callbacks_page());
            w.telegram = Some(self.telegram_page());
            w.voice = Some(self.voice_page());
            w.memory = Some(self.memory_page());
            w.journal_issues = Some(self.journal_issues_page(String::new()));
            w.games = Some(games_page(String::new()));
            w.chats = Some(chats_page(&self.root, String::new()));
            w.skills = Some(harness_session::skills::skills(&self.root, harness_session::skills::users_skills_dir().as_deref()).into_iter().map(|s| (s.name, s.description, s.from.to_string())).collect());
            w.profiles = Some(self.profiles_page(String::new()));
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
        // Pushed by the daemon on this window's connection (CON-130), subscribed again by the
        // client after a loss.
        let batches = client.subscribe("events", &window.window, 0);
        std::thread::spawn(move || {
            for batch in batches {
                for event in batch {
                    let Ok(numbered) = serde_json::from_value::<harness_daemon::Numbered>(event) else { continue };
                    if sender.send((tab, numbered.event)).is_err() {
                        return;
                    }
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
                    let what = if handoff.installed {
                        "You asked for a reload: the harness has been restarted on the installed binary, with no build since it is not a local build, and this conversation is here. Go on with what you were doing."
                    } else {
                        "You asked for a reload: the harness has been rebuilt and restarted with your changes, and this conversation is here. Go on with what you were doing."
                    };
                    self.sessions.inbound(id, harness_api::HARNESS_LABEL.to_string(), what.to_string());
                }
            }
        }
        w.active = handoff.active.min(w.tabs.len().saturating_sub(1));
        w.callbacks = Some(self.callbacks_page());
            w.telegram = Some(self.telegram_page());
            w.voice = Some(self.voice_page());
            w.memory = Some(self.memory_page());
            w.journal_issues = Some(self.journal_issues_page(String::new()));
            w.games = Some(games_page(String::new()));
            w.chats = Some(chats_page(&self.root, String::new()));
            w.skills = Some(harness_session::skills::skills(&self.root, harness_session::skills::users_skills_dir().as_deref()).into_iter().map(|s| (s.name, s.description, s.from.to_string())).collect());
            w.profiles = Some(self.profiles_page(String::new()));
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
            note(w, "Reloading…".to_string());
            changed = true;
        }
        if let Some(result) = self.reloading.as_ref().and_then(|r| r.try_recv().ok()) {
            self.reloading = None;
            changed = true;
            match result {
                Ok(built) => {
                    self.ready_to_swap = true;
                    self.installed_swap = built == reload::Built::Installed;
                    if self.installed_swap {
                        note(w, "Restarting on the installed binary: it is not a local build, so nothing is built (CON-129).".to_string());
                    }
                }
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
            installed: self.installed_swap,
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
        // The daemon holds the custom handlers, so one of another build is replaced too when it is
        // idle, and the new process starts this build's. When other windows' runs are on in it, it
        // is left as it is: this window's reload never waits on them (CON-130, journal#848), and
        // the new process works with it, since it speaks the same protocol.
        if let Some(client) = harness_daemon::client::Client::find(&dir) {
            // This window's own run may be starting there before it shows here: that one it waits for.
            if let Some(window) = self.sessions.window_id()
                && client.conversation_op(&serde_json::json!({"window": window, "op": "window_busy"})).is_ok_and(|r| r["busy"] == true)
            {
                return Ok(changed);
            }
            if client.build().is_some_and(|b| b != harness_daemon::build_on_disk()) {
                let _ = client.shutdown_if_idle();
            }
        }
        let path = reload::write_handoff(&dir, &handoff)?;
        reload::swap(&path, theme)?;
        Ok(true)
    }

    /// What the daemon said reaches the view, or the agent.
    /// CON-131: a toast every time the connection to the daemon is lost and every time one is made,
    /// the first included.
    fn receive_connection(&mut self, state: &mut frontend_tui::AppState) -> bool {
        use harness_daemon::client::Connection;
        if self.connection.is_none() {
            let client = self.sessions.client().or_else(|| self.window_ids.values().next().map(|(c, _)| c.clone()));
            self.connection = client.map(|c| c.watch());
        }
        // Only a session shows toasts: on the start page nothing is taken, so what happened there,
        // the first connection included, shows on the first session view (journal#873).
        let frontend_tui::model::Mode::Session(w) = &mut state.mode else { return false };
        if let Some(why) = LOCAL_REASON.lock().expect("reason").take() {
            w.toast = Some(frontend_tui::model::Toast { kind: frontend_tui::model::InboundKind::System, from: "daemon".into(), text: format!("This window runs its conversations itself: {why}.") });
            return true;
        }
        let now_ms = chrono::Utc::now().timestamp_millis() as u64;
        // The reconnecting toast's bar pulses: it is drawn again on every turn of the loop.
        let mut changed = match &mut w.reconnecting {
            Some(r) => {
                r.now_ms = now_ms;
                true
            }
            None => false,
        };
        let Some(watch) = &self.connection else { return changed };
        let toast = |text: String| frontend_tui::model::Toast { kind: frontend_tui::model::InboundKind::System, from: "daemon".into(), text };
        for event in watch.try_iter().collect::<Vec<_>>() {
            changed = true;
            match event {
                Connection::Connected { pid, again } => {
                    w.reconnecting = None;
                    w.toast = Some(toast(if again { format!("Reconnected to the daemon, pid {pid}.") } else { format!("Connected to the daemon, pid {pid}.") }));
                }
                Connection::Lost => {
                    w.reconnecting = Some(frontend_tui::model::Reconnecting { attempt: 0, reason: String::new(), now_ms });
                    w.toast = Some(toast("Connection to the daemon lost: trying again every 2 s.".to_string()));
                }
                Connection::Trying { attempt, reason } => {
                    w.reconnecting = Some(frontend_tui::model::Reconnecting { attempt, reason, now_ms });
                }
            }
        }
        changed
    }

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
                let factory: harness_session::Factory = std::sync::Arc::new(move |_, _| Ok(harness_scenarios::parts(harness_scenarios::setup(&name).expect("checked above"))));
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
            None => {
                let opened = harness_daemon::own_program()
                    .map_err(|e| e.to_string())
                    .and_then(|program| harness_session::remote::daemon_for_this_build(&config::data_dir().map_err(|e| e.to_string())?, &program))
                    .and_then(|client| harness_session::remote::RemoteSessions::open(client, &root, &link_settings, reload_asked.clone()));
                match opened {
                    Ok(remote) => Some(remote),
                    Err(why) => {
                        // Said once on screen, never silent (CON-130).
                        *LOCAL_REASON.lock().expect("reason") = Some(why);
                        None
                    }
                }
            }
        };
        let live = Live { sessions: remote.map_or(harness_session::remote::SessionLink::Local(local), harness_session::remote::SessionLink::Remote), syncing: Default::default(), previous_adapter: None, linking: None, reload_by_user: false, reload_for: None, shown_up_to: Default::default(), held_inbound: Vec::new(), context_chosen, checks_shown: None, plan_cost: Default::default(), rebind_after: Default::default(), toast_shown: None, login_cancel: None, login_events: std::sync::mpsc::channel(), journal_checks: std::sync::mpsc::channel(), chat_jobs: std::sync::mpsc::channel(), chat_job_next: 1, plan_models: std::sync::mpsc::channel(), step_debug_tabs: Default::default(), window_ids: Default::default(), prompt_opened: None, first_start_done: false, feeds: Default::default(), asks: Default::default(), context_window, root, real: script.is_none(), recent: Vec::new(), provider, model, budget, urls, windows: Default::default(), window_events: std::sync::mpsc::channel(), registered: Default::default(), reload_asked, reloading: None, ready_to_swap: false, swap_tried: None, voice_download: None, voice_test: None, voice_devices: None, terminal_wanted: None, local_env: None, installed_swap: false, connection: None };
        // What its conversations do if this window is lost (CON-181): the daemon holds it.
        live.sessions.set_on_loss(behaviour_now().loss().0);
        Ok(live)
    }

    fn fresh_session(&self) -> frontend_tui::AppState {
        let mut state = live_session(&self.provider, &self.model, &self.budget);
        if let frontend_tui::model::Mode::Session(w) = &mut state.mode {
            w.callbacks = Some(self.callbacks_page());
            w.telegram = Some(self.telegram_page());
            w.voice = Some(self.voice_page());
            w.memory = Some(self.memory_page());
            w.journal_issues = Some(self.journal_issues_page(String::new()));
            w.games = Some(games_page(String::new()));
            w.chats = Some(chats_page(&self.root, String::new()));
            w.skills = Some(harness_session::skills::skills(&self.root, harness_session::skills::users_skills_dir().as_deref()).into_iter().map(|s| (s.name, s.description, s.from.to_string())).collect());
            w.profiles = Some(self.profiles_page(String::new()));
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

    /// The Profiles tab as the profiles stand now (CON-125), with a line to say.
    fn profiles_page(&self, status: String) -> frontend_tui::model::ProfilesPage {
        let current = config::current();
        let rows = match config::overview() {
            Ok(lines) => lines
                .into_iter()
                .map(|l| frontend_tui::model::ProfileRow {
                    current: l.name == current,
                    folder: config::profile_dir(&l.name).map(|d| d.display().to_string()).unwrap_or_default(),
                    name: l.name,
                    provider: l.provider,
                    model: l.model,
                    claude: l.claude,
                    chatgpt: l.chatgpt,
                    kind: l.kind,
                    claude_account: l.claude_account,
                })
                .collect(),
            Err(e) => return frontend_tui::model::ProfilesPage { status: format!("The profiles cannot be read: {e:#}"), ..Default::default() },
        };
        frontend_tui::model::ProfilesPage { rows, status, ..Default::default() }
    }

    /// Use (CON-127): this window, every tab in it and the next bare start move onto `name`.
    fn use_profile(&mut self, w: &mut frontend_tui::model::Workspace, name: &str) -> Result<()> {
        let env = config::environment_for(harness_session::this_environment(), name)?;
        self.sessions.set_environment(env.clone()).map_err(|e| anyhow!(e))?;
        // Sessions this window runs itself take the profile's environment from the factory (journal#824).
        self.local_env = Some(std::sync::Arc::new(env));
        config::switch(name)?;
        // The next bare start runs on it too (CON-127).
        config::keep(name)?;
        let c = config::load()?;
        self.provider = c.session.provider.clone();
        self.model = c.session.model.clone();
        self.rebuild_factory(c.session)?;
        let label = profile_label();
        // A tab nothing was said in has no session yet: it opens on the new profile, its adapter and model.
        let unstarted = |t: &frontend_tui::model::Tab| !t.view.transcript.iter().any(|i| matches!(i, frontend_tui::model::Item::Turn { .. } | frontend_tui::model::Item::Tool(_) | frontend_tui::model::Item::Inbound(_)));
        for t in w.tabs.iter_mut() {
            frontend_tui::live::set_status(&mut t.view, "PROFILE", &label);
            if unstarted(t) {
                frontend_tui::live::set_status(&mut t.view, "ADAPTER", &self.provider);
                frontend_tui::live::set_status(&mut t.view, "MODEL", &self.model);
            }
        }
        // Every started tab moves too, whatever its adapter (CON-127): its session starts again by
        // its id on the factory that now has this profile's sign-ins, history and all; one whose run
        // is on moves when that run ends. It keeps its own adapter and model.
        for t in &w.tabs {
            if t.view.generating || !self.sessions.rebind(t.id) {
                self.rebind_after.insert(t.id);
            }
        }
        // Every settings page is the new profile's from now on, the open window included (journal#791).
        self.refresh_pages(w);
        Ok(())
    }

    /// The settings pages read again from this window's profile; an open settings window is drawn
    /// again from them, on the section it was on.
    fn refresh_pages(&self, w: &mut frontend_tui::model::Workspace) {
        w.auth = Some(self.auth_page(None));
        w.memory = Some(self.memory_page());
            w.journal_issues = Some(self.journal_issues_page(String::new()));
            w.games = Some(games_page(String::new()));
            w.chats = Some(chats_page(&self.root, String::new()));
            w.skills = Some(harness_session::skills::skills(&self.root, harness_session::skills::users_skills_dir().as_deref()).into_iter().map(|s| (s.name, s.description, s.from.to_string())).collect());
        if let Some(frontend_tui::model::Overlay::Settings(s)) = &w.overlay {
            let (section, focus, profiles) = (s.section, s.focus, s.profiles.clone());
            let mut fresh = frontend_tui::overlay::settings_for(w);
            (fresh.section, fresh.focus, fresh.profiles) = (section, focus, profiles);
            // On the Provider page the cursor starts on the profile's own adapter.
            fresh.cursor = fresh.provider;
            w.overlay = Some(frontend_tui::model::Overlay::Settings(fresh));
        }
    }

    /// The Journal page of this window's project (ADR 0028) from its files alone: gh is asked only by
    /// a check, off the window's thread, since it goes over the network (CON-132).
    fn journal_issues_page(&self, status: String) -> frontend_tui::model::JournalPage {
        journal_page_for(&self.root, status, false)
    }

    /// Checks gh for the Journal page on a thread of its own; the page comes back by journal_checks.
    fn check_journal_issues(&self, status: String) {
        let (root, sender) = (self.root.clone(), self.journal_checks.0.clone());
        std::thread::spawn(move || {
            let _ = sender.send(journal_page_for(&root, status, true));
        });
    }

    /// What a check of gh found reaches the Journal page, its cursor kept.
    /// The window quits: the way it leaves its conversations, asked or set, goes to the daemon,
    /// and a remembered choice becomes 'On exit' (CON-181).
    fn leave_now(&mut self, state: &mut frontend_tui::AppState) {
        let frontend_tui::model::Mode::Session(w) = &mut state.mode else { return };
        let Some(at) = w.outbox.iter().rposition(|o| matches!(o, frontend_tui::model::Outgoing::Leave { .. })) else { return };
        let frontend_tui::model::Outgoing::Leave { mode, remember } = w.outbox.remove(at) else { return };
        let Some(leave) = harness_session::remote::Leave::parse(&mode) else { return };
        if remember {
            let _ = config::load().and_then(|mut c| {
                c.behaviour.on_exit = Some(mode.clone());
                config::save(&c)
            });
        }
        self.sessions.leave(leave);
    }

    /// A Chats tab's dump or load: its progress moves its toast (CON-172); when it has finished the
    /// toast goes, the page shows the outcome with its list read again, and a toast says it when
    /// the Chats page is not open.
    fn tick_chat_jobs(&mut self, w: &mut frontend_tui::model::Workspace) -> bool {
        let mut changed = false;
        while let Ok(job) = self.chat_jobs.1.try_recv() {
            changed = true;
            match job {
                ChatJob::Progress { job, done, total } => {
                    if let Some(m) = w.archiving.iter_mut().find(|m| m.tab == job) {
                        (m.done, m.total) = (done, total);
                    }
                }
                ChatJob::Picked(picked) => {
                    if let Some(frontend_tui::model::Overlay::Settings(s)) = &mut w.overlay {
                        s.chats.picking = false;
                    }
                    if let Some(page) = &mut w.chats {
                        page.picking = false;
                    }
                    match picked {
                        file_picker::Picked::Path(path) => self.start_chat_load(w, path.to_string_lossy().into_owned()),
                        file_picker::Picked::Cancelled => self.chats_status(w, "No archive chosen.".to_string(), false),
                        // No picker here: the in-window list of archives instead.
                        file_picker::Picked::Unavailable(why) => self.chats_status(w, format!("{why}. Choose from the list, or type a path."), true),
                    }
                }
                ChatJob::Done { job, said } => {
                    w.archiving.retain(|m| m.tab != job);
                    let page = chats_page(&self.root, said.clone());
                    let on_chats = match &mut w.overlay {
                        Some(frontend_tui::model::Overlay::Settings(s)) => {
                            let cursor = s.chats.cursor.min(page.rows.len().saturating_sub(1));
                            let opened = s.chats.opened.filter(|&i| i < page.rows.len());
                            s.chats = frontend_tui::model::ChatsPage { cursor, opened, picking: s.chats.picking, ..page.clone() };
                            s.sections.get(s.section).is_some_and(|n| n == frontend_tui::overlay::CHATS_SECTION)
                        }
                        _ => false,
                    };
                    if !on_chats {
                        w.toast = Some(frontend_tui::overlay::system_toast(&said));
                    }
                    w.chats = Some(page);
                }
            }
        }
        changed
    }

    /// The Chats page says `said`; with `list`, it opens its in-window list of archives (CON-173).
    fn chats_status(&mut self, w: &mut frontend_tui::model::Workspace, said: String, list: bool) {
        let fresh = chats_page(&self.root, said);
        for page in [w.overlay.as_mut().and_then(|o| if let frontend_tui::model::Overlay::Settings(s) = o { Some(&mut s.chats) } else { None }), w.chats.as_mut()].into_iter().flatten() {
            page.status.clone_from(&fresh.status);
            page.archives.clone_from(&fresh.archives);
            if list {
                page.loading = true;
                page.load_cursor = 0;
            }
        }
    }

    /// A load of the Chats tab, with its toast (CON-172).
    fn start_chat_load(&mut self, w: &mut frontend_tui::model::Workspace, path: String) {
        let root = self.root.clone();
        let file = if std::path::Path::new(&path).is_absolute() { std::path::PathBuf::from(&path) } else { std::path::Path::new(&root).join(&path) };
        let shown = file.file_name().map_or_else(|| path.clone(), |n| n.to_string_lossy().into_owned());
        self.start_chat_job(w, format!("Loading {shown}"), move |progress| match config::data_dir() {
            Ok(data) => match harness_session::chat_archive::load_progress(&data.to_string_lossy(), &root, &file, &harness_session::chat_archive::LoadOptions { name: None }, progress) {
                Ok(l) => format!("Loaded {} as {}: paths moved from {}; journal {} written, {} already here. It is on the start page.", l.name, l.session, l.moved_from, l.journal_written, l.journal_merged),
                Err(e) => format!("Not loaded: {e}"),
            },
            Err(e) => format!("Not loaded: {e}"),
        });
    }

    /// Starts a dump or a load of the Chats tab off the window's thread, with its toast (CON-172).
    fn start_chat_job(&mut self, w: &mut frontend_tui::model::Workspace, what: String, run: impl FnOnce(&dyn Fn(u64, u64)) -> String + Send + 'static) {
        let job = self.chat_job_next;
        self.chat_job_next += 1;
        let now_ms = chrono::Utc::now().timestamp_millis() as u64;
        w.archiving.push(frontend_tui::model::Moving { tab: job, what, bytes: true, now_ms, ..Default::default() });
        let sender = self.chat_jobs.0.clone();
        std::thread::spawn(move || {
            let progress = |done, total| {
                let _ = sender.send(ChatJob::Progress { job, done, total });
            };
            let said = run(&progress);
            let _ = sender.send(ChatJob::Done { job, said });
        });
    }

    fn tick_journal_checks(&mut self, w: &mut frontend_tui::model::Workspace) -> bool {
        let Ok(page) = self.journal_checks.1.try_recv() else { return false };
        if let Some(frontend_tui::model::Overlay::Settings(s)) = &mut w.overlay {
            let cursor = s.journal.cursor;
            s.journal = frontend_tui::model::JournalPage { cursor, ..page.clone() };
        }
        w.journal_issues = Some(page);
        true
    }

    /// Saves the Journal page for this project; turning sending on queues its open todos (ADR 0028).
    fn save_journal_issues(&self, tracker: &str, gh_dir: &str, enabled: bool, public_ok: bool) -> Result<String, String> {
        use harness_daemon::trackers;
        let data = config::data_dir().map_err(|e| e.to_string())?.to_string_lossy().into_owned();
        let folder = harness_session::tracker_folder(&data, &self.root);
        let mut settings = trackers::read_settings(&folder);
        let was = settings.enabled;
        settings.tracker = Some(tracker.to_string());
        settings.gh_config_dir = (!gh_dir.trim().is_empty()).then(|| std::path::PathBuf::from(gh_dir.trim()));
        settings.enabled = enabled;
        settings.public_ok = public_ok;
        trackers::write_settings(&folder, &settings).map_err(|e| e.to_string())?;
        if enabled && !was {
            let todos = harness_session::open_todos(&data, &self.root)?;
            for id in &todos {
                trackers::enqueue(&folder, &trackers::Item { root: self.root.clone(), action: trackers::Action::Open { id: *id } }).map_err(|e| e.to_string())?;
            }
            return Ok(format!("Saved for this project: on, {} open todos queued.", todos.len()));
        }
        Ok(format!("Saved for this project: {}.", if enabled { "on" } else { "off" }))
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
        // This window's profile and its own Claude Code sign-in (journal#791).
        page.profile = config::current();
        page.claude_note = config::claude_note();
        let account = config::load().ok().and_then(|c| config::claude_account(&config::claude_binary(&c), &config::claude_dir(&config::current()).ok()?));
        page.claude_signed = account.as_ref().map(|a| a.signed_in);
        page.claude_account = account.as_ref().and_then(config::claude_account_words);
        // Codex in this profile's own folder (journal#913); none when it is not on this machine.
        page.codex_signed = config::load().ok().and_then(|c| config::codex_signed_in(&config::codex_binary(&c), &config::codex_dir(&config::current()).ok()?));
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
        frontend_tui::live::set_status(&mut t.view, "PROFILE", &profile_label());
        frontend_tui::live::set_status(&mut t.view, "MODEL", model);
        frontend_tui::live::show_context_window(&mut t.view, self.context_window);
        let has_session = self.sessions.session_of(tab).is_some();
        let name = t.title.clone();
        match self.sessions.choose(tab, provider, model) {
            Ok(true) if has_session => {
                self.syncing.insert(tab);
                start_moving(w, tab, &name, "Bringing the conversation in line", &provider_label(provider));
            }
            Ok(_) => {}
            Err(why) => t.view.transcript.push(frontend_tui::model::Item::Tips { label: "SETTINGS".into(), lines: vec![why] }),
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
            let plan = harness_auth_chatgpt::ChatGptAuth::with_store(harness_session_store(&dir));
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
            let mut host = host_for(note, self.reload_asked.clone(), selection_summary(&session));
            // Its profile's sign-ins, once the window moved off the one it started on (journal#824).
            host.environment = self.local_env.clone();
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
        if provider == "codex" {
            c.session.providers.entry("codex".to_string()).or_insert_with(|| serde_json::json!({"kind": "codex", "context_window": 272_000}));
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
            let name: String = info.title.chars().take(30).collect();
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let reports = std::sync::Mutex::new(tx.clone());
                harness_api::platform::progress::bind(harness_session::throttled(move |progress| {
                    if let Ok(to) = reports.lock() {
                        let _ = to.send(Linking::Progress(progress));
                    }
                }));
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
                let _ = tx.send(Linking::Done(linked));
            });
            self.linking = Some(rx);
            let mut state = self.fresh_session();
            if let frontend_tui::model::Mode::Session(w) = &mut state.mode {
                let tab = w.tabs[w.active].id;
                self.syncing.insert(tab);
                start_moving(w, tab, &name, "Bringing in the conversation", "");
            }
            return Some(state);
        }
        self.open_stored(info)
    }

    /// A conversation that was being linked is ready: it opens. Otherwise the window goes on.
    fn tick_linking(&mut self, state: &mut frontend_tui::AppState) {
        let mut done = None;
        while let Some(said) = self.linking.as_ref().and_then(|rx| rx.try_recv().ok()) {
            match said {
                Linking::Progress(progress) => {
                    if let frontend_tui::model::Mode::Session(w) = &mut state.mode
                        && let Some(m) = w.moving.first_mut()
                    {
                        m.what = progress.what;
                        m.done = progress.done;
                        m.total = progress.total;
                        m.bytes = progress.unit == harness_api::platform::progress::Unit::Bytes;
                    }
                }
                Linking::Done(result) => {
                    done = Some(result);
                    break;
                }
            }
        }
        let Some(done) = done else { return };
        self.linking = None;
        // The tab it was being brought into; others opened meanwhile stay (CON-144).
        let mut into = None;
        if let frontend_tui::model::Mode::Session(w) = &mut state.mode
            && let Some(at) = w.moving.iter().position(|m| self.syncing.contains(&m.tab) && w.tabs.iter().any(|t| t.id == m.tab))
        {
            let m = w.moving.remove(at);
            self.syncing.remove(&m.tab);
            into = Some(m.tab);
        }
        match done {
            Ok(info) => {
                if let Some(mut opened) = self.open_stored(info) {
                    let others = matches!(&state.mode, frontend_tui::model::Mode::Session(w) if w.tabs.len() > 1);
                    match (&mut state.mode, &mut opened.mode, into) {
                        // Only the placeholder tab is replaced by the conversation's.
                        (frontend_tui::model::Mode::Session(w), frontend_tui::model::Mode::Session(new), Some(tab)) if others => {
                            let moving = std::mem::take(&mut new.moving);
                            let fresh = new.tabs.swap_remove(new.active);
                            if let Some(at) = w.tabs.iter().position(|t| t.id == tab) {
                                w.tabs[at] = fresh;
                                w.active = at;
                            }
                            w.moving.extend(moving);
                        }
                        _ => *state = opened,
                    }
                }
            }
            Err(message) => {
                if let frontend_tui::model::Mode::Session(w) = &mut state.mode {
                    w.toast = Some(frontend_tui::overlay::system_toast(&format!("Could not bring the conversation in: {}", message.lines().next().unwrap_or_default())));
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
            let mut feed = frontend_tui::live::Feed::new(local_time, self.context_window);
            let view = w.view_mut();
            view.transcript.clear();
            // Line by line into the view: the log is never held whole beside it (journal#955).
            let (context_chosen, mut step_debug) = (self.context_chosen, None);
            let last = harness_session::each_logged(&data.to_string_lossy(), &self.root, &info.id, |entry| {
                if let harness_api::Event::StepDebug { on, .. } = entry.event {
                    step_debug = Some(on);
                }
                if !(context_chosen && matches!(entry.event, harness_api::Event::ContextWindow { .. })) {
                    feed.apply(view, &entry);
                }
            });
            if let Some(on) = step_debug {
                self.note_step_debug(tab, on);
            }
            if opened == harness_session::remote::Opened::Attached {
                view.generating = true;
            } else {
                feed.replayed(view);
            }
            self.shown_up_to.insert(tab, last.unwrap_or(0));
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
            frontend_tui::live::set_status(view, "PROFILE", &profile_label());
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
                let name = w.tabs[active].title.clone();
                start_moving(w, tab, &name, "Catching up with the conversation", "");
            }
        }
        Some(state)
    }

    /// What the user did goes to the sessions; returns whether anything changed on screen.
    fn send(&mut self, state: &mut frontend_tui::AppState) {
        use frontend_tui::model::{Mode, Outgoing, Overlay};
        let Mode::Session(w) = &mut state.mode else { return };
        // CON-131: while the window has no connection to the daemon, what the user sends is refused
        // at once and said so; a typed message goes back to the composer. Nothing waits for later.
        let down = self.sessions.client().is_some_and(|c| !c.current());
        for out in std::mem::take(&mut w.outbox) {
            let to_daemon = matches!(out, Outgoing::Prompt { .. } | Outgoing::Task { .. } | Outgoing::Answer { .. } | Outgoing::Stop { .. } | Outgoing::Plan { .. } | Outgoing::StepDebug { .. } | Outgoing::Nick { .. } | Outgoing::Unqueue { .. } | Outgoing::ProfileUse { .. });
            // CON-144: only a conversation being moved is held; what is sent into it is refused.
            let into = match &out {
                Outgoing::Prompt { tab, .. } | Outgoing::Task { tab, .. } | Outgoing::Answer { tab, .. } | Outgoing::Plan { tab, .. } => Some(*tab),
                _ => None,
            };
            if let Some(m) = into.and_then(|tab| w.moving.iter().find(|m| m.tab == tab)).cloned() {
                if let Outgoing::Prompt { tab, text } | Outgoing::Task { tab, text, .. } = &out
                    && let Some(t) = w.tabs.iter_mut().find(|t| t.id == *tab)
                {
                    t.view.composer.text = text.clone();
                    t.view.composer.back = 0;
                }
                w.toast = Some(frontend_tui::overlay::system_toast(&moving_refusal(&m)));
                continue;
            }
            if down && to_daemon {
                if let Outgoing::Prompt { tab, text } | Outgoing::Task { tab, text, .. } = &out
                    && let Some(t) = w.tabs.iter_mut().find(|t| t.id == *tab)
                {
                    t.view.composer.text = text.clone();
                    t.view.composer.back = 0;
                }
                w.toast = Some(frontend_tui::model::Toast { kind: frontend_tui::model::InboundKind::System, from: "daemon".into(), text: "Not sent: no connection to the daemon. Reconnecting; send it again once it is back.".into() });
                continue;
            }
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
                Outgoing::ProfileAdd { name } => {
                    let status = match config::create(&name) {
                        Ok(_) => format!("Made {name}: clean, nothing signed in. Use it, then choose its adapter on the Provider page."),
                        Err(e) => format!("Not made: {e:#}"),
                    };
                    w.profiles = Some(self.profiles_page(status));
                    show_profiles(w);
                }
                Outgoing::ProfileDelete { name } => {
                    let status = match config::delete(&name) {
                        Ok(()) => format!("Deleted {name}. Your conversations stay."),
                        Err(e) => format!("Not deleted: {e:#}"),
                    };
                    w.profiles = Some(self.profiles_page(status));
                    show_profiles(w);
                }
                Outgoing::ProfileUse { name } => {
                    let status = match self.use_profile(w, &name) {
                        Ok(()) => format!("This window is on {name} now: new tabs run on it; tabs already started keep theirs."),
                        Err(e) => format!("Still on {}: {e:#}", config::current()),
                    };
                    w.profiles = Some(self.profiles_page(status));
                    w.memory = Some(self.memory_page());
            w.journal_issues = Some(self.journal_issues_page(String::new()));
            w.games = Some(games_page(String::new()));
            w.chats = Some(chats_page(&self.root, String::new()));
            w.skills = Some(harness_session::skills::skills(&self.root, harness_session::skills::users_skills_dir().as_deref()).into_iter().map(|s| (s.name, s.description, s.from.to_string())).collect());
                    show_profiles(w);
                }
                Outgoing::ProfileClaude { name, login } => self.terminal_wanted = Some((name, if login { ClaudeStep::Login } else { ClaudeStep::Logout })),
                Outgoing::ProfileClaudeSwitch { name } => self.terminal_wanted = Some((name, ClaudeStep::Switch)),
                Outgoing::ProfileCodex { name, login } => self.terminal_wanted = Some((name, if login { ClaudeStep::CodexLogin } else { ClaudeStep::CodexLogout })),
                Outgoing::JournalCheck => self.check_journal_issues("Checking gh...".to_string()),
                // CON-171: a dump or a load runs off the window's thread; its outcome comes back.
                Outgoing::ChatDump { session } => {
                    let root = self.root.clone();
                    let name = w.chats.as_ref().and_then(|p| p.rows.iter().find(|r| r.session == session)).map_or_else(|| session.clone(), |r| r.name.clone());
                    self.start_chat_job(w, format!("Dumping {name}"), move |progress| match config::data_dir() {
                        Ok(data) => match harness_session::chat_archive::dump_progress(&data.to_string_lossy(), &root, &session, None, progress) {
                            Ok(d) => format!("Dumped {} to {}: {:.1} MB{}.", d.name, d.path.display(), d.bytes as f64 / 1e6, if d.journal { ", with the journal and its maps" } else { ", without the journal (none could be read)" }),
                            Err(e) => format!("Not dumped: {e}"),
                        },
                        Err(e) => format!("Not dumped: {e}"),
                    });
                }
                Outgoing::ChatLoad { path } => self.start_chat_load(w, path),
                // CON-173: the system's file picker, off the window's thread.
                Outgoing::ChatPick => {
                    let dumps = std::path::Path::new(&self.root).join(harness_session::chat_archive::DUMPS_DIR);
                    let start = if dumps.is_dir() { dumps } else { std::path::PathBuf::from(&self.root) };
                    let sender = self.chat_jobs.0.clone();
                    std::thread::spawn(move || {
                        let _ = sender.send(ChatJob::Picked(file_picker::pick(&start)));
                    });
                }
                // CON-181: Settings > Behaviour saved; the daemon hears the new loss at once.
                Outgoing::SaveBehaviour { on_exit, on_loss } => {
                    let said = config::load().and_then(|mut c| {
                        c.behaviour = config::Behaviour { on_exit: Some(on_exit.clone().unwrap_or_else(|| "ask".to_string())), on_loss: Some(on_loss.clone()) };
                        config::save(&c).map(|_| c.behaviour)
                    });
                    match said {
                        Ok(b) => {
                            w.on_exit = b.exit().0.map(|l| l.as_str().to_string());
                            self.sessions.set_on_loss(b.loss().0);
                            w.behaviour = Some(behaviour_page(&b, "Saved.".to_string()));
                        }
                        Err(e) => w.behaviour = Some(behaviour_page(&behaviour_now(), format!("Not saved: {e}"))),
                    }
                    if let (Some(frontend_tui::model::Overlay::Settings(s)), Some(page)) = (&mut w.overlay, &w.behaviour) {
                        s.behaviour = page.clone();
                    }
                }
                // Done where the window quits (leave_now): it is the last thing it does.
                Outgoing::Leave { .. } => {}
                Outgoing::SaveGames { enabled, texas_holdem } => {
                    let said = config::data_dir().map_err(|e| e.to_string()).and_then(|data| {
                        let settings = harness_daemon::games::GameSettings { enabled, texas_holdem, ..harness_daemon::games::read_settings(&data) };
                        harness_daemon::games::write_settings(&data, &settings).map_err(|e| e.to_string())
                    });
                    let page = games_page(match said {
                        Ok(()) => format!("Saved: games are {}.", if enabled { "on: make a table on the dashboard (`multitool dashboard`)" } else { "off" }),
                        Err(e) => format!("Not saved: {e}"),
                    });
                    if let Some(frontend_tui::model::Overlay::Settings(s)) = &mut w.overlay {
                        let cursor = s.games.cursor;
                        s.games = frontend_tui::model::GamesPage { cursor, ..page.clone() };
                    }
                    w.games = Some(page);
                }
                Outgoing::JournalConnect => {
                    if let Ok(data) = config::data_dir() {
                        let folder = harness_session::tracker_folder(&data.to_string_lossy(), &self.root);
                        let dir = harness_daemon::trackers::gh_dir(&folder, &harness_daemon::trackers::read_settings(&folder));
                        self.terminal_wanted = Some((dir.to_string_lossy().into_owned(), ClaudeStep::GhLogin));
                    }
                }
                Outgoing::SaveJournalIssues { tracker, gh_dir, enabled, public_ok } => {
                    let said = self.save_journal_issues(&tracker, &gh_dir, enabled, public_ok).unwrap_or_else(|e| format!("Not saved: {e}"));
                    self.check_journal_issues(said.clone());
                    let page = self.journal_issues_page(said);
                    if let Some(frontend_tui::model::Overlay::Settings(s)) = &mut w.overlay {
                        let cursor = s.journal.cursor;
                        s.journal = frontend_tui::model::JournalPage { cursor, ..page.clone() };
                    }
                    w.journal_issues = Some(page);
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
            w.journal_issues = Some(self.journal_issues_page(String::new()));
            w.games = Some(games_page(String::new()));
            w.chats = Some(chats_page(&self.root, String::new()));
            w.skills = Some(harness_session::skills::skills(&self.root, harness_session::skills::users_skills_dir().as_deref()).into_iter().map(|s| (s.name, s.description, s.from.to_string())).collect());
            w.profiles = Some(self.profiles_page(String::new()));
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
            w.journal_issues = Some(self.journal_issues_page(String::new()));
            w.games = Some(games_page(String::new()));
            w.chats = Some(chats_page(&self.root, String::new()));
            w.skills = Some(harness_session::skills::skills(&self.root, harness_session::skills::users_skills_dir().as_deref()).into_iter().map(|s| (s.name, s.description, s.from.to_string())).collect());
            w.profiles = Some(self.profiles_page(String::new()));
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
            w.journal_issues = Some(self.journal_issues_page(String::new()));
            w.games = Some(games_page(String::new()));
            w.chats = Some(chats_page(&self.root, String::new()));
            w.skills = Some(harness_session::skills::skills(&self.root, harness_session::skills::users_skills_dir().as_deref()).into_iter().map(|s| (s.name, s.description, s.from.to_string())).collect());
            w.profiles = Some(self.profiles_page(String::new()));
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
            w.journal_issues = Some(self.journal_issues_page(String::new()));
            w.games = Some(games_page(String::new()));
            w.chats = Some(chats_page(&self.root, String::new()));
            w.skills = Some(harness_session::skills::skills(&self.root, harness_session::skills::users_skills_dir().as_deref()).into_iter().map(|s| (s.name, s.description, s.from.to_string())).collect());
            w.profiles = Some(self.profiles_page(String::new()));
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
            w.journal_issues = Some(self.journal_issues_page(String::new()));
            w.games = Some(games_page(String::new()));
            w.chats = Some(chats_page(&self.root, String::new()));
            w.skills = Some(harness_session::skills::skills(&self.root, harness_session::skills::users_skills_dir().as_deref()).into_iter().map(|s| (s.name, s.description, s.from.to_string())).collect());
            w.profiles = Some(self.profiles_page(String::new()));
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
                    // CON-144: a conversation being moved is not moved again until it is done.
                    if let Some(m) = w.moving.iter().find(|m| m.tab == w.tabs[w.active].id) {
                        w.toast = Some(frontend_tui::overlay::system_toast(&moving_refusal(m)));
                        continue;
                    }
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
                            let model = if model.is_empty() { "its own default model".to_string() } else { model.clone() };
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
                    frontend_tui::live::set_status(&mut opened.view, "PROFILE", &profile_label());
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
                FromSession::AskWithdrawn { tab, call_id, approved } => {
                    // Answered elsewhere (CON-163): its card goes, shown or queued, and a key
                    // pressed now answers nothing of it; a toast says how (journal#1188).
                    withdraw_ask(&mut w.overlay, &mut self.asks, tab, &call_id);
                    w.toast = Some(frontend_tui::model::Toast { kind: frontend_tui::model::InboundKind::System, from: "telegram".into(), text: withdrawn_toast(approved).to_string() });
                    if let Some(a) = w.tabs.iter_mut().find(|t| t.id == tab).and_then(|t| t.view.activity.as_mut())
                        && a.state == "Waiting for your answer"
                    {
                        a.state = if approved { "Approved on Telegram" } else { "Rejected on Telegram" }.to_string();
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
                FromSession::Progress { tab, progress } => {
                    if let Some(m) = w.moving.iter_mut().find(|m| m.tab == tab) {
                        m.what = progress.what;
                        m.done = progress.done;
                        m.total = progress.total;
                        m.bytes = progress.unit == harness_api::platform::progress::Unit::Bytes;
                    }
                }
                FromSession::Synced { tab, error } => {
                    self.syncing.remove(&tab);
                    w.moving.retain(|m| m.tab != tab);
                    if let Some(message) = &error {
                        let first = message.lines().next().unwrap_or_default();
                        w.toast = Some(frontend_tui::overlay::system_toast(&format!("Could not move the conversation: {first}. It stays where it was.")));
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
                    w.moving.retain(|m| m.tab != tab);
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

/// This profile's ChatGPT credential store (CON-122); the data directory's if it cannot be named.
fn harness_session_store(data_dir: &std::path::Path) -> harness_auth_chatgpt::Store {
    config::chatgpt_dir().map_or_else(|_| harness_auth_chatgpt::Store::new(data_dir), |dir| harness_auth_chatgpt::Store::at(&dir))
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

/// What the terminal is handed to `claude auth` for, on a profile's Claude Code directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClaudeStep {
    Login,
    Logout,
    /// Out, then in again with another account.
    Switch,
    /// Codex's own sign-in and sign-out, in the profile's Codex folder (journal#913).
    CodexLogin,
    CodexLogout,
    /// gh's own sign-in, in a project's gh folder (ADR 0028); the name carries the folder.
    GhLogin,
}

/// The open settings show the Profiles tab as it now stands, the line under the cursor kept.
fn show_profiles(w: &mut frontend_tui::model::Workspace) {
    let Some(page) = w.profiles.clone() else { return };
    if let Some(frontend_tui::model::Overlay::Settings(s)) = &mut w.overlay {
        let cursor = s.profiles.cursor.min(page.rows.len().saturating_sub(1));
        s.profiles = frontend_tui::model::ProfilesPage { cursor, ..page };
    }
}

/// The status line's word for this window's profile, with where its Claude Code runs when that is
/// not its own (CON-120).
/// `multitool daemon restart|stop` (CON-128): stops the daemon this data directory has, whatever
/// it is doing, and with `start` starts the installed build's, which takes the port it had.
fn daemon_restart(start: bool, only_pid: Option<u32>) -> Result<()> {
    let dir = config::data_dir()?;
    let running = harness_daemon::client::Client::find(&dir);
    let old = running.as_ref().map(|old| {
        let stats = old.stats().unwrap_or_default();
        (stats["pid"].as_u64().map_or("?".to_string(), |p| p.to_string()), stats["conversations"]["busy"].as_u64().unwrap_or(0), old.port())
    });
    // The service's stop: another daemon holding the folder is not the service's to stop.
    if let (Some(want), Some((pid, ..))) = (only_pid, &old)
        && *pid != want.to_string()
    {
        println!("the daemon running is pid {pid}, not {want}: left as it is");
        return Ok(());
    }
    let runs = |busy: u64| match busy {
        0 => "no run was going on".to_string(),
        1 => "1 window's run was stopped".to_string(),
        n => format!("{n} windows' runs were stopped"),
    };
    // Run by a user service (CON-162): a restart is one step of it, so no window can start a
    // daemon of its own in between (journal#1181). `stop` stays below: it is the service's ExecStop.
    if start && let Some(kind) = harness_daemon::service::installed_for(&dir) {
        // One running outside the service makes way first, cleanly.
        if let Some(old) = &running
            && !harness_daemon::service::active(kind)
        {
            old.shutdown().map_err(|e| anyhow!(e))?;
        }
        harness_daemon::service::restart(kind).map_err(|e| anyhow!("the daemon's service would not restart: {e}"))?;
        if let Some((pid, busy, port)) = &old {
            println!("stopped the daemon (pid {pid}, port {port}): {}", runs(*busy));
        }
        let until = std::time::Instant::now() + std::time::Duration::from_secs(20);
        let new = loop {
            if let Some(client) = harness_daemon::client::Client::find(&dir)
                && let Ok(stats) = client.stats()
                && old.as_ref().is_none_or(|(pid, ..)| stats["pid"].as_u64().map(|p| p.to_string()).as_deref() != Some(pid))
            {
                break (client, stats);
            }
            if std::time::Instant::now() > until {
                bail!("the service was restarted, and no new daemon answered in 20 s: see `systemctl --user status {}`", harness_daemon::service::UNIT);
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        };
        let pid = new.1["pid"].as_u64().map_or("?".to_string(), |p| p.to_string());
        println!("started the daemon through its service (pid {pid}, port {}): open windows reopen their conversations in it", new.0.port());
        return Ok(());
    }
    if let (Some(old), Some((pid, busy, port))) = (&running, &old) {
        old.shutdown().map_err(|e| anyhow!(e))?;
        println!("stopped the daemon (pid {pid}, port {port}): {}", runs(*busy));
    } else {
        println!("no daemon was running");
    }
    if !start {
        return Ok(());
    }
    let program = harness_daemon::own_program()?;
    let new = harness_daemon::client::Client::find_or_start(&dir, &program).map_err(|e| anyhow!(e))?;
    let pid = new.stats().ok().and_then(|s| s["pid"].as_u64()).map_or("?".to_string(), |p| p.to_string());
    println!("started the daemon (pid {pid}, port {}): open windows reopen their conversations in it", new.port());
    Ok(())
}

/// A hand-off of one tab, for `--resume <session>` (its conversation) or `--new` (none yet).
fn one_tab_handoff(session: Option<String>) -> Result<PathBuf> {
    let handoff = reload::Handoff {
        active: 0,
        theme: "dark".into(),
        tabs: vec![reload::TabHandoff { title: if session.is_some() { "resumed".into() } else { "new".into() }, session, window: None, url: None, scroll: 0, draft: String::new() }],
        tell: None,
        installed: false,
    };
    let dir = config::data_dir()?.join("opened");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("handoff-{}.json", std::process::id()));
    std::fs::write(&path, serde_json::to_string(&handoff)?)?;
    Ok(path)
}

/// The toast of a plan answered on Telegram (CON-163, journal#1188).
fn withdrawn_toast(approved: bool) -> &'static str {
    if approved { "✅ План одобрен в Telegram" } else { "✏️ План не одобрен в Telegram" }
}

/// A question answered elsewhere (CON-163): its card goes, shown or queued, and the next queued
/// one shows in its place. Nothing else is touched.
fn withdraw_ask(overlay: &mut Option<frontend_tui::model::Overlay>, asks: &mut std::collections::VecDeque<frontend_tui::model::PermissionView>, tab: u64, call_id: &str) {
    let it = |v: &frontend_tui::model::PermissionView| v.tab == Some(tab) && v.call_id == call_id;
    asks.retain(|v| !it(v));
    if matches!(overlay, Some(frontend_tui::model::Overlay::Permission(v)) if it(v)) {
        *overlay = asks.pop_front().map(frontend_tui::model::Overlay::Permission);
    }
}


/// Why this window could not have its conversations in the daemon, to say once on screen.
static LOCAL_REASON: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

fn profile_label() -> String {
    // Short: the status line holds more after it. The full note is on the Profiles tab and in `profile list`.
    match config::claude_note() {
        Some(_) => format!("{} (your Claude)", config::current()),
        None => config::current(),
    }
}

/// `multitool login|logout claude-code`: the unmodified `claude auth login|logout`, run in this
/// profile's Claude Code directory with the terminal handed to it (CON-123). Nothing of the
/// harness's own Claude Code environment reaches it, and the harness reads nothing of what it saves.
/// `multitool login|logout codex`: the unmodified `codex login` (or `codex logout`) in this profile's
/// own Codex folder (journal#913), as Claude Code's sign-in is (CON-123). The harness reads,
/// stores and passes on none of what Codex saves there.
fn codex_auth(profile: &str, login: bool) -> Result<()> {
    let c = config::load_profile(profile)?;
    let binary = config::codex_binary(&c);
    let dir = config::codex_dir(profile)?;
    config::prepare_codex(&dir, &config::users_codex_dir()?)?;
    let what = if login { "login" } else { "logout" };
    println!("Profile {profile}: running `{binary} {what}` in {}", dir.display());
    if login {
        println!("In the browser, sign in with the ChatGPT account this profile is for.");
    }
    let status = std::process::Command::new(&binary).arg(what).env("CODEX_HOME", &dir).status().with_context(|| format!("running {binary}"))?;
    if !status.success() {
        bail!("`{binary} {what}` ended with {status}");
    }
    let signed = config::codex_signed_in(&binary, &dir);
    println!("Profile {profile}: Codex {}", match signed {
        Some(true) => "signed in",
        Some(false) => "not signed in",
        None => "cannot be asked",
    });
    Ok(())
}

fn claude_code_auth(what: &str) -> Result<()> {
    claude_code_auth_for(&config::current(), what)
}

/// The same for a named profile: the Profiles tab's sign-in buttons.
fn claude_code_auth_for(profile: &str, what: &str) -> Result<()> {
    let c = config::load_profile(profile)?;
    let binary = config::claude_binary(&c);
    let dir = config::claude_dir(profile)?;
    config::prepare_claude(&dir, &config::users_claude_dir()?)?;
    let mut command = std::process::Command::new(&binary);
    command.args(["auth", what]);
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("CLAUDE") {
            command.env_remove(name);
        }
    }
    println!("Profile {profile}: running `{binary} auth {what}` in {}", dir.display());
    if what == "logout" {
        println!("This signs out only this profile's own Claude Code login; your other logins on this machine stay.");
    }
    if what == "login" {
        // The browser approves with whatever account it is signed in to (journal#802).
        println!("In the browser, sign in with the account this profile is for. If the browser is signed in to claude.ai");
        println!("with another account it will approve that one: press c to copy the address and open it in a private");
        println!("window, or sign out of claude.ai in the browser first.");
    }
    let status = command.env("CLAUDE_CONFIG_DIR", &dir).status().with_context(|| format!("running {binary}"))?;
    if !status.success() {
        bail!("`{binary} auth {what}` ended with {status}");
    }
    let signed = config::claude_signed_in(&binary, &dir);
    println!("Claude Code for the profile {profile}: {}", match signed { Some(true) => "signed in", Some(false) => "not signed in", None => "status unknown" });
    Ok(())
}

/// `multitool profile list`.
fn list_profiles() -> Result<()> {
    for name in config::list()? {
        let c = config::load_profile(&name)?;
        let claude = match config::claude_account(&config::claude_binary(&c), &config::claude_dir(&name)?) {
            Some(account) => config::claude_account_words(&account).unwrap_or_else(|| "not signed in".to_string()),
            None => "unknown".to_string(),
        };
        let chatgpt = if harness_auth_chatgpt::status(&harness_auth_chatgpt::Store::at(&config::profile_dir(&name)?.join("chatgpt"))).0 { "signed in" } else { "not signed in" };
        let mark = if name == config::current() { "*" } else { " " };
        println!("{mark} {name:<16} {:<14} {:<24} Claude Code: {claude:<32} ChatGPT: {chatgpt}", c.session.provider, c.session.model);
    }
    Ok(())
}

fn only_chatgpt(provider: &str) -> Result<()> {
    match provider {
        "chatgpt" => Ok(()),
        "claude" | "anthropic" => bail!("the harness takes no Claude subscription token (CON-56): sign this profile's Claude Code in with `multitool login claude-code`, which runs Claude Code's own sign-in, or set ANTHROPIC_API_KEY with a key from Claude Console"),
        other => bail!("no sign-in for {other:?}: chatgpt or claude-code"),
    }
}

/// `multitool login chatgpt`: the user signs in on OpenAI's page; nothing here sees a password.
fn login(provider: &str) -> Result<()> {
    only_chatgpt(provider)?;
    let store = harness_session_store(&config::data_dir()?);
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
            FromSession::Synced { .. } | FromSession::Progress { .. } | FromSession::AskWithdrawn { .. } => {}
            FromSession::Unavailable { message, .. } => bail!("{message}"),
            FromSession::Spawn { .. } | FromSession::CloseTab { .. } | FromSession::Queued { .. } => {}
        }
    }
}

/// The Games page, from the daemon's games settings (ADR 0029).
/// The Chats page of this window's project (CON-171): its conversations by adapter, and the .zip
/// files in its folder to load.
/// What a Chats tab's dump or load tells the window (CON-172).
enum ChatJob {
    Progress { job: u64, done: u64, total: u64 },
    Done { job: u64, said: String },
    /// What the system's file picker gave, for a load (CON-173).
    Picked(file_picker::Picked),
}

fn chats_page(root: &str, status: String) -> frontend_tui::model::ChatsPage {
    let Ok(data) = config::data_dir() else { return frontend_tui::model::ChatsPage { status, ..Default::default() } };
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
    let age = |ms: u64| {
        let s = now.saturating_sub(ms) / 1000;
        match s {
            0..=3599 => format!("{} min ago", s / 60),
            3600..=86_399 => format!("{} h ago", s / 3600),
            _ => format!("{} days ago", s / 86_400),
        }
    };
    let mut rows: Vec<frontend_tui::model::ChatRow> = harness_session::chat_archive::listed(&data.to_string_lossy(), root)
        .into_iter()
        .map(|(s, adapter, bytes)| frontend_tui::model::ChatRow { session: s.id, name: s.title, adapter, bytes, age: age(s.last_ms) })
        .collect();
    // Grouped by adapter; within one the newest first, as listed.
    rows.sort_by(|a, b| a.adapter.cmp(&b.adapter));
    // The dumps' folder first, then older dumps left in the project's root (journal#1288).
    let zips = |dir: &std::path::Path| -> Vec<String> {
        let mut found: Vec<String> = std::fs::read_dir(dir).map(|d| d.flatten().filter_map(|e| e.file_name().to_str().filter(|n| n.ends_with(".zip")).map(str::to_string)).collect()).unwrap_or_default();
        found.sort();
        found
    };
    let dumps = harness_session::chat_archive::DUMPS_DIR;
    let mut archives: Vec<String> = zips(&std::path::Path::new(root).join(dumps)).into_iter().map(|n| format!("{dumps}/{n}")).collect();
    archives.extend(zips(std::path::Path::new(root)));
    frontend_tui::model::ChatsPage { rows, archives, status, ..Default::default() }
}

fn games_page(status: String) -> frontend_tui::model::GamesPage {
    let settings = config::data_dir().map(|data| harness_daemon::games::read_settings(&data)).unwrap_or_default();
    frontend_tui::model::GamesPage { enabled: settings.enabled, texas_holdem: settings.texas_holdem, status, ..Default::default() }
}

/// The Journal page of a project (ADR 0028). With `check`, gh is asked who it is and what the
/// repository is: network calls, never on the window's thread (CON-132).
fn journal_page_for(root: &str, status: String, check: bool) -> frontend_tui::model::JournalPage {
    use harness_daemon::trackers;
    let Ok(data) = config::data_dir() else { return Default::default() };
    let folder = harness_session::tracker_folder(&data.to_string_lossy(), root);
    let settings = trackers::read_settings(&folder);
    let place = harness_daemon::git_origin(root).as_deref().and_then(trackers::github_place);
    let tracker = trackers::tracker(&folder, &settings);
    let (account, visibility, why) = if check {
        (
            tracker.connected().unwrap_or_else(|e| format!("not connected: {e}")),
            place.as_deref().and_then(|p| tracker.visibility(p).ok()).unwrap_or_default(),
            trackers::why_not(&settings, place.as_deref(), tracker.as_ref()).unwrap_or_default(),
        )
    } else {
        let why = if settings.enabled { String::new() } else { "off: 'Send todos to issues' is not turned on".to_string() };
        ("not checked yet: r checks gh".to_string(), String::new(), why)
    };
    frontend_tui::model::JournalPage {
        project: root.to_string(),
        tracker: frontend_tui::model::TRACKERS.iter().position(|(n, _)| Some(*n) == settings.tracker.as_deref()).unwrap_or(0),
        gh_dir: settings.gh_config_dir.as_ref().map(|d| d.to_string_lossy().into_owned()).unwrap_or_default(),
        gh_dir_used: folder.join("gh").to_string_lossy().into_owned(),
        account,
        place: place.unwrap_or_default(),
        visibility,
        enabled: settings.enabled,
        public_ok: settings.public_ok,
        why,
        queued: std::fs::read_to_string(folder.join("queue.jsonl")).map_or(0, |t| t.lines().count()),
        issues: trackers::issues(&folder).len(),
        status,
        ..Default::default()
    }
}

/// `multitool journal issues` (ADR 0028): a project's tracker settings, per project (CON-142).
fn journal_issues(data: &str, root: &str, action: IssuesAction) -> Result<()> {
    use harness_daemon::trackers;
    let folder = harness_session::tracker_folder(data, root);
    let mut settings = trackers::read_settings(&folder);
    let place = harness_daemon::git_origin(root).as_deref().and_then(trackers::github_place);
    match action {
        IssuesAction::Status => {}
        IssuesAction::Tracker { name } => {
            if !trackers::TRACKERS.contains(&name.as_str()) {
                bail!("no tracker {name:?}; there are: {}", trackers::TRACKERS.join(", "));
            }
            settings.tracker = Some(name);
            trackers::write_settings(&folder, &settings)?;
        }
        IssuesAction::GhDir { path } => {
            settings.gh_config_dir = (path != "-").then(|| std::path::PathBuf::from(path));
            trackers::write_settings(&folder, &settings)?;
        }
        IssuesAction::Connect => {
            let dir = trackers::gh_dir(&folder, &settings);
            std::fs::create_dir_all(&dir)?;
            let binary = settings.gh_binary.clone().unwrap_or_else(|| "gh".to_string());
            println!("gh auth login, in {} (GH_CONFIG_DIR)", dir.display());
            let status = std::process::Command::new(&binary).args(["auth", "login"]).env("GH_CONFIG_DIR", &dir).status().with_context(|| format!("running {binary}"))?;
            if !status.success() {
                bail!("gh auth login did not finish");
            }
        }
        IssuesAction::Off => {
            settings.enabled = false;
            trackers::write_settings(&folder, &settings)?;
        }
        IssuesAction::On { public } => {
            let tracker = trackers::tracker(&folder, &settings);
            let place = place.clone().ok_or_else(|| anyhow!("this project has no GitHub remote: git remote get-url origin"))?;
            tracker.connected().map_err(|e| anyhow!("not connected: {e}. Run `multitool journal issues connect`, or `gh-dir` with an account's gh folder"))?;
            let visibility = tracker.visibility(&place).map_err(|e| anyhow!("{place}: {e}"))?;
            if visibility.starts_with("PUBLIC") && !public && !settings.public_ok {
                bail!("{place} is public: its issues are seen by everyone. Run `multitool journal issues on --public` to send its todos there anyway");
            }
            let was = settings.enabled;
            settings.enabled = true;
            settings.public_ok |= public;
            trackers::write_settings(&folder, &settings)?;
            if !was {
                let todos = harness_session::open_todos(data, root).map_err(|e| anyhow!(e))?;
                for id in &todos {
                    trackers::enqueue(&folder, &trackers::Item { root: root.to_string(), action: trackers::Action::Open { id: *id } })?;
                }
                println!("on: {} open todos queued for {place}; the daemon sends them", todos.len());
            }
        }
    }
    let tracker = trackers::tracker(&folder, &settings);
    println!("project:  {root}");
    println!("tracker:  {}{}", tracker.name(), if tracker.name() == "github" { String::new() } else { " (not implemented yet)".to_string() });
    println!("gh:       {}", trackers::gh_dir(&folder, &settings).display());
    println!("as:       {}", tracker.connected().unwrap_or_else(|e| format!("not connected ({e})")));
    println!("place:    {}", place.as_deref().unwrap_or("none: no GitHub remote"));
    println!("sending:  {}", match trackers::why_not(&settings, place.as_deref(), tracker.as_ref()) {
        None => "on".to_string(),
        Some(why) => why,
    });
    let queued = std::fs::read_to_string(folder.join("queue.jsonl")).map_or(0, |t| t.lines().count());
    println!("queued:   {queued}; issues opened: {}", trackers::issues(&folder).len());
    if let Ok(last) = std::fs::read_to_string(folder.join("last-error.txt")) {
        println!("last error: {last}");
    }
    Ok(())
}

/// `multitool import`: lists a source's conversations, or brings one in as a session here.
/// A dump's or a load's progress on one line of stderr, when it is a terminal (CON-172).
fn progress_line(what: &'static str) -> impl Fn(u64, u64) {
    use std::io::IsTerminal;
    let shown = std::io::stderr().is_terminal();
    move |done, total| {
        if shown {
            let share = (done.min(total) * 100).checked_div(total).unwrap_or(0);
            eprint!("\r{what}: {:.1} / {:.1} MB · {share}%   ", done as f64 / 1e6, total as f64 / 1e6);
        }
    }
}

fn end_progress_line() {
    use std::io::IsTerminal;
    if std::io::stderr().is_terminal() {
        eprintln!();
    }
}

/// `multitool chat dump|load|list` (CON-171): what the Chats tab runs, on the command line.
fn chat(action: ChatAction) -> Result<()> {
    use harness_session::chat_archive;
    let root = std::env::current_dir()?.to_string_lossy().into_owned();
    let data = config::data_dir()?.to_string_lossy().into_owned();
    match action {
        ChatAction::Dump { conversation, out } => {
            let session = match conversation {
                Some(wanted) => chat_archive::resolve(&data, &root, &wanted).map_err(|e| anyhow!(e))?,
                None => harness_session::recent(&data, &root).into_iter().next().map(|s| s.id).ok_or_else(|| anyhow!("this folder has no conversation to dump"))?,
            };
            let dumped = chat_archive::dump_progress(&data, &root, &session, out.as_deref(), &progress_line("Dumping"));
            end_progress_line();
            let dumped = dumped.map_err(|e| anyhow!(e))?;
            println!("dumped {} ({session}) to {}: {:.1} MB, {} files{}", dumped.name, dumped.path.display(), dumped.bytes as f64 / 1e6, dumped.files, if dumped.journal { ", the journal with its maps in it" } else { "; the journal is NOT in it (none could be read)" });
            println!("on the other machine, from the project's folder: multitool chat load {}", dumped.path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned()));
        }
        ChatAction::Load { archive, name } => {
            let loaded = chat_archive::load_progress(&data, &root, &archive, &chat_archive::LoadOptions { name }, &progress_line("Loading"));
            end_progress_line();
            let loaded = loaded.map_err(|e| anyhow!(e))?;
            println!("loaded {} as {}{}", loaded.name, loaded.session, loaded.claude_id.as_deref().map_or_else(String::new, |id| format!(", Claude Code's {id}")));
            println!("project paths moved from {} to {root}", loaded.moved_from);
            println!("journal: {} entries written, {} already here", loaded.journal_written, loaded.journal_merged);
            println!("open it from the start page here, or: multitool --resume {}", loaded.session);
        }
        ChatAction::List => {
            for (s, adapter, bytes) in chat_archive::listed(&data, &root) {
                println!("{:<12} {:>8.1} MB  {}  {}", adapter, bytes as f64 / 1e6, s.id, s.title);
            }
        }
    }
    Ok(())
}

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
        // The terminal tab's name as last set (CON-170), and the project folder it carries.
        let mut last_title = String::new();
        let mut settings_were_open = false;
        let title_folder = std::env::current_dir().ok().and_then(|d| d.file_name().map(|n| n.to_string_lossy().into_owned())).unwrap_or_default();
        loop {
            // A profile's Claude Code sign-in: Claude Code's own, in the terminal, then back here (CON-123).
            if let Some((profile, step)) = live.terminal_wanted.take() {
                let _ = execute!(std::io::stdout(), DisableMouseCapture, DisableBracketedPaste);
                ratatui::restore();
                let (program, what) = match step {
                    ClaudeStep::Login => ("Claude Code", "login"),
                    ClaudeStep::Logout => ("Claude Code", "logout"),
                    ClaudeStep::Switch => ("Claude Code", "account switch"),
                    ClaudeStep::CodexLogin => ("Codex", "login"),
                    ClaudeStep::CodexLogout => ("Codex", "logout"),
                    ClaudeStep::GhLogin => ("gh", "auth login"),
                };
                let done = match step {
                    ClaudeStep::Logout => claude_code_auth_for(&profile, "logout"),
                    ClaudeStep::Login => claude_code_auth_for(&profile, "login"),
                    // Its own login out, then in again: the other logins on this machine stay (journal#802).
                    ClaudeStep::Switch => claude_code_auth_for(&profile, "logout").and_then(|()| claude_code_auth_for(&profile, "login")),
                    ClaudeStep::CodexLogin => codex_auth(&profile, true),
                    ClaudeStep::CodexLogout => codex_auth(&profile, false),
                    ClaudeStep::GhLogin => {
                        println!("gh auth login, in {profile} (GH_CONFIG_DIR)");
                        let _ = std::fs::create_dir_all(&profile);
                        match std::process::Command::new("gh").args(["auth", "login"]).env("GH_CONFIG_DIR", &profile).status() {
                            Ok(s) if s.success() => Ok(()),
                            Ok(s) => Err(anyhow!("gh auth login ended: {s}")),
                            Err(e) => Err(anyhow!("running gh: {e}")),
                        }
                    }
                };
                let said = match done {
                    Ok(()) => format!("{program} {what} for {profile} done."),
                    Err(e) => format!("{program} {what} for {profile}: {e:#}"),
                };
                println!("\n{said} Press Enter to go back to the harness.");
                let mut line = String::new();
                let _ = std::io::stdin().read_line(&mut line);
                terminal = ratatui::init();
                // A new terminal draws the whole frame; what the sign-in printed is cleared first, by the
                // terminal itself: Terminal::clear asks the cursor's position, which not every terminal answers.
                execute!(std::io::stdout(), ratatui::crossterm::terminal::Clear(ratatui::crossterm::terminal::ClearType::All), EnableMouseCapture, EnableBracketedPaste)?;
                if let frontend_tui::model::Mode::Session(w) = &mut state.mode {
                    // This window's own profile signed in or out: its new tabs run on what it now has
                    // (default leaves the user's own Claude Code once its own is signed in).
                    if profile == config::current() {
                        let moved = config::environment_for(harness_session::this_environment(), &profile).map_err(|e| e.to_string()).and_then(|env| {
                            live.sessions.set_environment(env.clone())?;
                            live.local_env = Some(std::sync::Arc::new(env));
                            Ok(())
                        });
                        if moved.is_ok() && let Ok(c) = config::load() {
                            let _ = live.rebuild_factory(c.session);
                        }
                    }
                    w.profiles = Some(live.profiles_page(said));
                    show_profiles(w);
                    live.refresh_pages(w);
                }
                dirty = true;
            }
            state.pictures = picker.is_some();
            live.connect_windows(&mut state);
            live.tick_linking(&mut state);
            dirty |= live.receive(&mut state);
            dirty |= live.receive_daemon(&mut state);
            dirty |= live.receive_connection(&mut state);
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
                // Whether its conversations can outlive the window: the exit dialog says so (CON-181).
                w.in_daemon = live.sessions.in_daemon();
                // A moving conversation's bar moves (CON-144).
                if !w.moving.is_empty() || !w.archiving.is_empty() {
                    let now_ms = chrono::Utc::now().timestamp_millis() as u64;
                    w.moving.iter_mut().chain(w.archiving.iter_mut()).for_each(|m| m.now_ms = now_ms);
                    dirty = true;
                }
                // Messages go to the tab in front, once there is one; a run on takes them after it.
                // One being moved takes them once it is done (CON-144).
                let front = w.tabs.get(w.active).map(|t| t.id);
                let waiting_for_daemon = w.moving.iter().any(|m| Some(m.tab) == front);
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
                dirty |= live.tick_journal_checks(w);
                dirty |= live.tick_chat_jobs(w);
                w.new_tab_runs_on = Some((live.provider.clone(), live.model.clone()));
                dirty |= live.tick_voice_download(w);
                dirty |= live.tick_voice_test(w);
                dirty |= live.tick_voice_devices(w);
            }
            // Input still queued (a burst of wheel events, fast typing) is handled first and
            // drawn once after, instead of one frame per event.
            if !event::poll(std::time::Duration::ZERO)? {
                // The Chats tab lists what is in the project now, each time the settings open
                // (CON-171): the conversations of the window's run are in it.
                let settings_open = matches!(&state.mode, frontend_tui::model::Mode::Session(w) if matches!(w.overlay, Some(frontend_tui::model::Overlay::Settings(_))));
                if settings_open && !settings_were_open
                    && let frontend_tui::model::Mode::Session(w) = &mut state.mode
                {
                    let page = chats_page(&live.root, String::new());
                    if let Some(frontend_tui::model::Overlay::Settings(s)) = &mut w.overlay {
                        s.chats = page.clone();
                    }
                    w.chats = Some(page);
                    dirty = true;
                }
                settings_were_open = settings_open;
                // The terminal's tab is named after the active tab's agent (CON-170), with a moon
                // while the window is busy (CON-174); written only when that changed.
                let title = frontend_tui::model::terminal_title(&state, &title_folder, chrono::Utc::now().timestamp_millis() as u64);
                if title != last_title {
                    let _ = execute!(std::io::stdout(), ratatui::crossterm::terminal::SetTitle(&title));
                    last_title = title;
                }
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
                            live.leave_now(&mut state);
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
                // A sideways wheel or touchpad swipe, or Shift with the wheel, over a map pans it
                // (journal#886); anywhere else the wheel scrolls the transcript.
                Event::Mouse(m) if matches!(m.kind, MouseEventKind::ScrollLeft | MouseEventKind::ScrollRight) => {
                    let step = if m.kind == MouseEventKind::ScrollRight { input::PAN_STEP } else { -input::PAN_STEP };
                    input::pan_at(&mut state, &drawn, m.column, m.row, step);
                    continue;
                }
                Event::Mouse(m) if matches!(m.kind, MouseEventKind::ScrollUp | MouseEventKind::ScrollDown) => {
                    let step = if m.kind == MouseEventKind::ScrollDown { input::PAN_STEP } else { -input::PAN_STEP };
                    if !(m.modifiers.contains(KeyModifiers::SHIFT) && input::pan_at(&mut state, &drawn, m.column, m.row, step)) {
                        input::scroll(&mut state, if m.kind == MouseEventKind::ScrollUp { 3 } else { -3 });
                    }
                    continue;
                }
                _ => continue,
            };
            match outcome {
                Outcome::Quit => {
                    live.leave_now(&mut state);
                    return Ok(());
                }
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

#[cfg(test)]
mod withdraw_tests {
    use super::*;
    use frontend_tui::model::{Overlay, PermissionView};

    fn card(tab: u64, call_id: &str) -> PermissionView {
        PermissionView { tab: Some(tab), call_id: call_id.to_string(), ..frontend_tui::overlay::sample_permission() }
    }

    /// CON-163: a card answered elsewhere goes, shown or queued; the next shows; others stay.
    #[test]
    fn a_withdrawn_card_goes_and_no_other() {
        let mut overlay = Some(Overlay::Permission(card(1, "plan-ask-7")));
        let mut asks: std::collections::VecDeque<PermissionView> = [card(2, "c9"), card(1, "plan-ask-8")].into_iter().collect();
        withdraw_ask(&mut overlay, &mut asks, 1, "plan-ask-8");
        assert!(matches!(&overlay, Some(Overlay::Permission(v)) if v.call_id == "plan-ask-7"), "the shown one is another call");
        assert_eq!(asks.iter().map(|v| v.call_id.as_str()).collect::<Vec<_>>(), ["c9"], "the queued one went");
        withdraw_ask(&mut overlay, &mut asks, 1, "plan-ask-7");
        assert!(matches!(&overlay, Some(Overlay::Permission(v)) if v.call_id == "c9"), "the next queued shows");
        assert!(asks.is_empty());
        withdraw_ask(&mut overlay, &mut asks, 3, "c9");
        assert!(overlay.is_some(), "another tab's call id withdraws nothing");
    }

    /// journal#1188: the toast says the answer given on Telegram.
    #[test]
    fn the_toast_says_the_answer() {
        assert_eq!(withdrawn_toast(true), "✅ План одобрен в Telegram");
        assert_eq!(withdrawn_toast(false), "✏️ План не одобрен в Telegram");
    }
}
