//! Modal windows over a session: the permission prompt (permission-*: a tool call waiting for
//! the user, IF-5) and the settings window (settings-overlay-*). Drawn after everything else;
//! while one is open it takes the keys. The dark mockups' terminal forms are followed in both
//! themes: `( )` radios, `[x]` boxes, `[ value ▾ ]` fields, framed buttons.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};

use crate::input::{Key, Outcome};
use crate::model::{AuthPage, DELEGATE_PROVIDER, InboundKind, ModelChoice, PLAN_PROVIDER, Toast};
use crate::model::{
    Item, Overlay, PermissionView, ProviderOption, Role, SettingsPane, SettingsView, Tone, ToolBody, ToolCall, ToolIcon, ToolState, Workspace, Outgoing, CallbacksPage, StandardRow, CustomRow, ConditionRow,
};
use crate::theme::Theme;
use crate::view::{Drawn, Hit, Target};

const CHOICES: [&str; 3] = ["Allow once", "Always allow", "Deny"];
const MODELS: [&str; 3] = ["claude-opus-5-5", "claude-sonnet-5-5", "claude-haiku-5-5"];
const TIMEOUTS: [u32; 4] = [30, 60, 120, 300];

pub fn sample_permission() -> PermissionView {
    PermissionView {
        tool: "shell".into(),
        summary: "An agent wants to execute a shell command on your machine.".into(),
        command: "cargo test -p harness-router".into(),
        workdir: "~/work/harness-multitool".into(),
        scope: "current workspace · local shell".into(),
        policy: "approval required · shell:execute".into(),
        choice: 0,
        tab: None,
        locked: false,
    }
}

/// The settings window, opening on what the active session's status line says.
pub fn settings_for(w: &Workspace) -> SettingsView {
    let mut s = sample_settings();
    if let Some(page) = &w.callbacks {
        s.callbacks = page.clone();
    }
    if let Some(page) = &w.auth {
        s.auth = page.clone();
    }
    if s.auth.delegate {
        let at = s.providers.iter().position(|p| p.name == CLAUDE_SUBSCRIPTION).unwrap_or(s.providers.len());
        s.providers.insert(at, ProviderOption { name: DELEGATE_PROVIDER.into(), description: "runs your signed-in Claude Code as an agent".into(), local_build: false, disabled: None });
    }
    // Yesterday's outcome is not news; an address still waiting to be opened is.
    if !s.auth.pending {
        s.auth.message.clear();
    }
    for (key, value) in &w.view().status.fields {
        match key.as_str() {
            "MODEL" => s.model = value.clone(),
            "ADAPTER" => {
                let name = match value.as_str() {
                    "anthropic" | "anthropic-api" => "Anthropic API",
                    "openai" | "openai-compatible" => "OpenAI-compatible",
                    "local" => "Local",
                    "chatgpt" => PLAN_PROVIDER,
                    "claude-code" => DELEGATE_PROVIDER,
                    other => other,
                };
                if let Some(i) = s.providers.iter().position(|p| p.name == name) {
                    s.provider = i;
                }
            }
            _ => {}
        }
    }
    s.saved = (s.provider, s.model.clone());
    s.context = chosen_context(&s);
    s.saved_context = s.context;
    (s.delegate_own, s.delegate_skip) = s.auth.delegate_row;
    s.saved_delegate = s.auth.delegate_row;
    s
}

/// What the context window can be set to; none is the model's own.
const CONTEXT_WINDOWS: [Option<u64>; 10] = [None, Some(32_000), Some(64_000), Some(128_000), Some(200_000), Some(272_000), Some(400_000), Some(500_000), Some(1_000_000), Some(2_000_000)];

fn context_label(window: Option<u64>) -> String {
    match window {
        None => "Auto".to_string(),
        Some(tokens) if tokens >= 1_000_000 => format!("{}M", tokens / 1_000_000),
        Some(tokens) => format!("{}k", tokens / 1000),
    }
}

/// What was saved for the provider row chosen.
fn chosen_context(s: &SettingsView) -> Option<u64> {
    let id = match s.providers[s.provider].name.as_str() {
        "Anthropic API" => "anthropic",
        "OpenAI-compatible" => "openai",
        "Local" => "local",
        PLAN_PROVIDER => "chatgpt",
        DELEGATE_PROVIDER => "claude-code",
        other => other,
    };
    s.auth.contexts.iter().find(|(row, _)| row == id).map(|(_, tokens)| *tokens)
}

pub fn sample_settings() -> SettingsView {
    let p = |name: &str, description: &str, local_build: bool| ProviderOption { name: name.into(), description: description.into(), local_build, disabled: None };
    SettingsView {
        sections: ["Provider", "Tools", "Policy", "Memory", "Router", "Frontend", "Callbacks", "Theme"].iter().map(|s| s.to_string()).collect(),
        section: 0,
        focus: SettingsPane::Sections,
        providers: vec![
            p("Anthropic API", "Claude, official API", false),
            p("OpenAI-compatible", "OpenAI, Azure, Groq, OpenRouter, ...", false),
            p("Local", "Ollama, vLLM, LM Studio, ...", false),
            p(PLAN_PROVIDER, "your ChatGPT plan, through gpt-subscription", false),
            ProviderOption { name: CLAUDE_SUBSCRIPTION.into(), description: CLAUDE_REASON.into(), local_build: false, disabled: Some(CLAUDE_REASON.into()) },
        ],
        provider: 0,
        cursor: 0,
        model: MODELS[0].into(),
        timeout_s: 60,
        fallback: true,
        callbacks: CallbacksPage::default(),
        auth: AuthPage::default(),
        editing_model: false,
        model_menu: None,
        saved: (0, MODELS[0].into()),
        run_on: false,
        context: None,
        context_menu: None,
        saved_context: None,
        delegate_own: true,
        delegate_skip: false,
        saved_delegate: (true, false),
    }
}

/// Shown so the choice is visible; nothing implements it (CON-56).
const CLAUDE_SUBSCRIPTION: &str = "claude-subscription";
const CLAUDE_REASON: &str = "not available: Anthropic's terms";

/// The harness's own message, as a toast.
pub fn system_toast(text: &str) -> Toast {
    Toast { kind: InboundKind::System, from: "settings".into(), text: text.into() }
}

/// The design sample of the Auth tab.
pub fn sample_auth() -> AuthPage {
    AuthPage {
        keys: vec![("Anthropic API".into(), "not set".into()), ("OpenAI-compatible".into(), "from OPENAI_API_KEY".into()), ("Local".into(), "none needed".into())],
        plan_status: "Signed in as you@example.com".into(),
        signed_in: true,
        models: vec![ModelChoice { id: "gpt-5.5".into(), detail: "in your plan · 272k context".into(), context: Some(272_000) }, ModelChoice { id: "gpt-5.5-mini".into(), detail: "in your plan · 128k context".into(), context: Some(128_000) }],
        ..Default::default()
    }
}

/// The harness's own callback types, each with the implementations this build has.
pub fn standard_rows() -> Vec<StandardRow> {
    let row = |name: &str, description: &str, concept| StandardRow {
        name: name.into(),
        description: description.into(),
        implementations: vec!["built-in".into(), "off".into()],
        chosen: 0,
        concept,
    };
    vec![
        row("notification", "into the agent's input", false),
        row("journal", "writes a journal entry", false),
        row("flowstate_update", "moves a flowchart instance", false),
        row("skill", "starts a skill run", true),
    ]
}

/// The design sample of the Callbacks tab.
pub fn sample_callbacks() -> CallbacksPage {
    let mut standard = standard_rows();
    standard[0].implementations.insert(1, "acme-notifier (local build)".into());
    standard[0].chosen = 1;
    standard[2].implementations.insert(1, "acme-flowstate (local build)".into());
    let custom = |name: &str, implementation: &str, on| CustomRow { name: name.into(), implementation: implementation.into(), on };
    let condition = |name: &str, listens_to: &str, rule: &str, policy: &str, on| ConditionRow { name: name.into(), listens_to: listens_to.into(), rule: rule.into(), policy: policy.into(), on };
    CallbacksPage {
        standard,
        custom: vec![custom("deploy_done", "acme-deploy", true), custom("nightly_report", "acme-reports", false)],
        conditions: vec![
            condition("pipeline keeps failing", "iteration_failed", "3 signals, 60 s quiet", ".harness/policies/pipeline.md", true),
            condition("tests are flaky", "test_failed", "5 in 10 min", ".harness/policies/flaky-tests.md", false),
        ],
        daemon: "running on 127.0.0.1:42849".into(),
        cursor: 0,
        open: None,
    }
}

/// The Callbacks tab (settings-callbacks-*): one list under three headings, rows at one cursor.
fn callbacks_page(f: &mut Frame, r: Rect, s: &SettingsView, t: &Theme, bg: Color, out: &mut Drawn) {
    let p = &s.callbacks;
    let focus = s.focus == SettingsPane::Page;
    let muted = Style::default().fg(t.text_muted);
    let strong = Style::default().fg(t.text).add_modifier(Modifier::BOLD);
    let heading = Style::default().fg(t.accent).add_modifier(Modifier::BOLD);
    let mut y = r.y;
    let bottom = r.y + r.height;
    let mut index = 0usize;
    let row = |f: &mut Frame, y: &mut u16, spans: Vec<Span<'static>>, index: &mut usize, out: &mut Drawn| {
        if *y >= bottom {
            return;
        }
        let under = focus && p.cursor == *index;
        text(f, r.x, *y, r.width, spans, if under { t.surface_raised } else { bg });
        out.hits.push(Hit { rect: Rect::new(r.x, *y, r.width, 1), target: Target::CallbackRow(*index) });
        *y += 1;
        *index += 1;
    };
    text(f, r.x, y, r.width, vec![Span::styled("Standard types", heading)], bg);
    y += 1;
    for (i, std) in p.standard.iter().enumerate() {
        let chosen = std.implementations.get(std.chosen).cloned().unwrap_or_default();
        let name_w = 18;
        let mut spans = vec![Span::styled(format!(" {:<name_w$}", std.name), strong)];
        let choice = format!("{} {}", chosen, if p.open == Some(i) { "▴" } else { "▾" });
        spans.push(Span::styled(format!("{choice:<31}"), Style::default().fg(t.text)));
        if std.concept {
            spans.push(Span::styled("concept · ", muted));
        }
        // What it does comes last: it is what a narrow window can do without.
        spans.push(Span::styled(std.description.clone(), muted));
        row(f, &mut y, spans, &mut index, out);
        if p.open == Some(i) {
            for (j, name) in std.implementations.iter().enumerate() {
                if y < bottom {
                    let mark = if j == std.chosen { "✓" } else { " " };
                    let style = if j == std.chosen { Style::default().fg(t.accent) } else { Style::default().fg(t.text) };
                    text(f, r.x, y, r.width, vec![Span::raw(" ".repeat(19)), Span::styled(format!("{mark} {name}"), style)], bg);
                    y += 1;
                }
            }
        }
    }
    let toggle = |on: bool| if on { Span::styled(" ● on  ", Style::default().fg(t.success)) } else { Span::styled(" ○ off ", muted) };
    y += 1;
    if y < bottom {
        text(f, r.x, y, r.width, vec![Span::styled("Custom callbacks", heading)], bg);
        y += 1;
    }
    if p.custom.is_empty() && y < bottom {
        text(f, r.x, y, r.width, vec![Span::styled(" none in this build", muted)], bg);
        y += 1;
    }
    for c in &p.custom {
        let spans = vec![toggle(c.on), Span::styled(format!("{:<24}", c.name), strong), Span::styled(c.implementation.clone(), muted)];
        row(f, &mut y, spans, &mut index, out);
    }
    y += 1;
    if y < bottom {
        text(f, r.x, y, r.width, vec![Span::styled("Conditions", heading), Span::styled("  .harness/conditions.toml", muted)], bg);
        y += 1;
    }
    if p.conditions.is_empty() && y < bottom {
        text(f, r.x, y, r.width, vec![Span::styled(" none in this project", muted)], bg);
        y += 1;
    }
    for c in &p.conditions {
        let spans = vec![toggle(c.on), Span::styled(format!("{:<24}", c.name), strong), Span::styled(c.listens_to.clone(), muted)];
        row(f, &mut y, spans, &mut index, out);
        if y < bottom {
            let policy = if c.policy.is_empty() { String::new() } else { format!(" → {}", c.policy) };
            text(f, r.x, y, r.width, vec![Span::raw("        "), Span::styled(format!("{}{policy}", c.rule), muted)], bg);
            y += 1;
        }
    }
    y += 1;
    if y < bottom {
        text(f, r.x, y, r.width, vec![Span::styled("Daemon  ", strong), Span::styled(p.daemon.clone(), muted), Span::styled("   r reload", muted)], bg);
    }
}

/// Space or Enter on the Callbacks tab: unfold or pick an implementation, flip a toggle.
fn activate_callback(p: &mut CallbacksPage) {
    let (standard, custom) = (p.standard.len(), p.custom.len());
    match p.cursor {
        i if i < standard => p.open = if p.open == Some(i) { None } else { Some(i) },
        i if i < standard + custom => p.custom[i - standard].on = !p.custom[i - standard].on,
        i if i < p.rows() => p.conditions[i - standard - custom].on = !p.conditions[i - standard - custom].on,
        _ => {}
    }
}

/// Fills `r` with `bg`, frame cells included, and draws a rounded frame in `line` around it.
/// Nothing under it shows through: a frame cell keeping the background of what lay under it let
/// the half-block edges of the cards behind appear along the outline.
fn modal(f: &mut Frame, r: Rect, line: Color, bg: Color) -> Rect {
    let buf = f.buffer_mut();
    for y in r.y..r.y + r.height {
        for x in r.x..r.x + r.width {
            if buf.area.contains((x, y).into()) {
                buf[(x, y)].reset();
                buf[(x, y)].set_bg(bg);
            }
        }
    }
    let block = Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(Style::default().fg(line));
    let inner = block.inner(r);
    f.render_widget(block, r);
    let buf = f.buffer_mut();
    for y in inner.y..inner.y + inner.height {
        for x in inner.x..inner.x + inner.width {
            buf[(x, y)].set_symbol(" ").set_style(Style::default().bg(bg));
        }
    }
    inner
}

fn text(f: &mut Frame, x: u16, y: u16, w: u16, spans: Vec<Span<'static>>, bg: Color) {
    f.render_widget(Paragraph::new(Line::from(spans)).style(Style::default().bg(bg)), Rect::new(x, y, w, 1));
}

fn rule(f: &mut Frame, x: u16, y: u16, w: u16, t: &Theme, bg: Color) {
    text(f, x, y, w, vec![Span::styled("─".repeat(w as usize), Style::default().fg(t.border_subtle))], bg);
}

/// `key label · key label ...` with the keys in bold, centred.
fn hints(f: &mut Frame, r: Rect, items: &[(&str, &str)], t: &Theme, bg: Color) {
    let mut spans = Vec::new();
    for (i, (k, l)) in items.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("   ", Style::default()));
        }
        spans.push(Span::styled(k.to_string(), Style::default().fg(t.text).add_modifier(Modifier::BOLD)));
        spans.push(Span::styled(format!(" {l}"), Style::default().fg(t.text_muted)));
    }
    f.render_widget(Paragraph::new(Line::from(spans)).centered().style(Style::default().bg(bg)), r);
}

pub fn draw(f: &mut Frame, area: Rect, o: &Overlay, t: &Theme, out: &mut Drawn) {
    match o {
        Overlay::Permission(p) => permission(f, area, p, t, out),
        Overlay::Settings(s) => settings(f, area, s, t, out),
        Overlay::Syncing(s) => syncing(f, area, s, t),
    }
}

/// The moon, what is being done and for how long. It answers nothing: it ends when the daemon says.
fn syncing(f: &mut Frame, area: Rect, s: &crate::model::SyncingView, t: &Theme) {
    let width = (s.what.chars().count() as u16 + 16).min(area.width);
    let r = Rect::new(area.x + area.width.saturating_sub(width) / 2, area.y + area.height.saturating_sub(5) / 2, width, 5.min(area.height));
    let inner = modal(f, r, t.accent_line, t.surface);
    let time = crate::widgets::run_time(s.now_ms.saturating_sub(s.started_ms));
    let spans = vec![
        Span::styled(format!(" {} ", crate::widgets::moon(s.now_ms)), Style::default().fg(t.accent)),
        Span::styled(s.what.clone(), Style::default().fg(t.text).add_modifier(Modifier::BOLD)),
        Span::styled(format!("  {time}"), Style::default().fg(t.accent)),
    ];
    text(f, inner.x, inner.y + inner.height / 2, inner.width, spans, t.surface);
}

/// A framed button 3 rows tall; the focused one is filled (half-block edges keep the fill
/// inside its outline).
fn button(f: &mut Frame, r: Rect, label: &str, color: Color, focused: bool, t: &Theme, bg: Color) {
    if focused {
        let buf = f.buffer_mut();
        let (right, bottom) = (r.x + r.width - 1, r.y + r.height - 1);
        for y in r.y..=bottom {
            for x in r.x..=right {
                let sym = match (x == r.x, x == right, y == r.y, y == bottom) {
                    (true, _, true, _) => "▗",
                    (_, true, true, _) => "▖",
                    (true, _, _, true) => "▝",
                    (_, true, _, true) => "▘",
                    (_, _, true, _) => "▄",
                    (_, _, _, true) => "▀",
                    (true, _, _, _) => "▐",
                    (_, true, _, _) => "▌",
                    _ => " ",
                };
                if sym == " " {
                    buf[(x, y)].set_symbol(" ").set_style(Style::default().bg(color));
                } else {
                    buf[(x, y)].set_symbol(sym).set_style(Style::default().fg(color).bg(bg));
                }
            }
        }
        let fg = if t.dark { t.background } else { t.accent_text };
        f.render_widget(
            Paragraph::new(Span::styled(label.to_string(), Style::default().fg(fg).add_modifier(Modifier::BOLD))).centered().style(Style::default().bg(color)),
            Rect::new(r.x + 1, r.y + 1, r.width - 2, 1),
        );
    } else {
        let inner = modal(f, r, color, bg);
        let fg = if color == t.border_subtle { t.text } else { color };
        f.render_widget(Paragraph::new(Span::styled(label.to_string(), Style::default().fg(fg).add_modifier(Modifier::BOLD))).centered(), inner);
    }
}

fn permission(f: &mut Frame, area: Rect, p: &PermissionView, t: &Theme, out: &mut Drawn) {
    let bg = t.surface;
    let w = 76.min(area.width.saturating_sub(4));
    // Rows inside the frame: title 2, gap, summary, gap, command 3, gap, table 5, gap,
    // buttons 3, note, gap, keys.
    let h = (2 + 1 + 1 + 1 + 3 + 1 + 5 + 1 + 3 + 1 + 1 + 1 + 2u16).min(area.height);
    let r = Rect::new(area.x + (area.width - w) / 2, area.y + area.height.saturating_sub(h) / 2, w, h);
    let inner = modal(f, r, t.accent_line, bg);
    let (x, iw) = (inner.x + 2, inner.width.saturating_sub(4));
    let mut y = inner.y;
    let stripe = Style::default().fg(t.accent_line);
    text(f, x, y, iw, vec![Span::styled("▌ ", stripe), Span::styled("Permission required", Style::default().fg(t.text).add_modifier(Modifier::BOLD))], bg);
    text(f, x, y + 1, iw, vec![Span::styled("▌ ", stripe), Span::styled(format!("tool: {}", p.tool), Style::default().fg(t.text_muted))], bg);
    y += 3;
    text(f, x, y, iw, vec![Span::styled(p.summary.clone(), Style::default().fg(t.text))], bg);
    y += 2;
    // The command in a frame of its own.
    let cmd = modal(f, Rect::new(x, y, iw, 3), t.border_subtle, t.surface_raised);
    text(
        f,
        cmd.x,
        cmd.y,
        cmd.width,
        vec![
            Span::styled(" $ ", Style::default().fg(t.text_muted)),
            Span::styled("│ ", Style::default().fg(t.border_subtle)),
            Span::styled(p.command.clone(), Style::default().fg(t.text).add_modifier(Modifier::BOLD)),
        ],
        t.surface_raised,
    );
    y += 4;
    // Where and under which policy.
    let rows = [("WORKDIR", &p.workdir), ("SCOPE", &p.scope), ("POLICY", &p.policy)];
    let table = modal(f, Rect::new(x, y, iw, rows.len() as u16 + 2), t.border_subtle, bg);
    for (i, (k, v)) in rows.iter().enumerate() {
        text(
            f,
            table.x + 1,
            table.y + i as u16,
            table.width - 2,
            vec![Span::styled(format!("{k:<12}"), Style::default().fg(t.text_muted).add_modifier(Modifier::BOLD)), Span::styled(v.to_string(), Style::default().fg(t.text))],
            bg,
        );
    }
    y += rows.len() as u16 + 3;
    // Allow once, Always allow, Deny.
    let gap = 2u16;
    let bw = (iw - gap * 2) / 3;
    for (i, label) in CHOICES.iter().enumerate() {
        let color = match i {
            0 => t.accent_line,
            1 => t.border_subtle,
            _ => t.error,
        };
        let br = Rect::new(x + i as u16 * (bw + gap), y, bw, 3);
        button(f, br, label, color, p.choice == i, t, bg);
        out.hits.push(Hit { rect: br, target: Target::Choice(i) });
    }
    y += 3;
    text(f, x, y, iw, vec![Span::styled(if p.locked { "One moment: keys that were meant for the composer cannot answer this." } else { "Always allow creates a persistent rule for this command pattern." }, Style::default().fg(t.text_muted).add_modifier(Modifier::ITALIC))], bg);
    hints(
        f,
        Rect::new(inner.x, y + 2, inner.width, 1),
        &[("1", "once"), ("2", "always"), ("3", "deny"), ("←→", "choose"), ("Enter", "confirm"), ("Esc", "deny")],
        t,
        bg,
    );
}

fn settings(f: &mut Frame, area: Rect, s: &SettingsView, t: &Theme, out: &mut Drawn) {
    let bg = t.surface;
    let w = 104.min(area.width.saturating_sub(4));
    // A sign-in address is long: the window grows for it, as far as the terminal lets it.
    let h = (if s.auth.pending { 36u16 } else { 30u16 }).min(area.height);
    let r = Rect::new(area.x + (area.width - w) / 2, area.y + area.height.saturating_sub(h) / 2, w, h);
    let inner = modal(f, r, t.border_subtle, bg);
    let (x, iw) = (inner.x + 2, inner.width.saturating_sub(4));
    text(f, x, inner.y, iw, vec![Span::styled("Settings", Style::default().fg(t.text).add_modifier(Modifier::BOLD))], bg);
    text(f, x, inner.y + 1, iw, vec![Span::styled("project and local build configuration", Style::default().fg(t.text_muted))], bg);
    let close = Rect::new(inner.x + inner.width - 3, inner.y, 1, 1);
    text(f, close.x, close.y, 1, vec![Span::styled("×", Style::default().fg(t.text_muted))], bg);
    out.hits.push(Hit { rect: close, target: Target::CloseOverlay });
    rule(f, inner.x, inner.y + 2, inner.width, t, bg);
    let body_y = inner.y + 4;
    let footer_y = inner.y + inner.height - 2;
    // Sections.
    let side_w = 24u16;
    for (i, name) in s.sections.iter().enumerate() {
        let y = body_y + i as u16;
        if y + 1 >= footer_y {
            break;
        }
        let selected = i == s.section;
        let row = Rect::new(x, y, side_w, 1);
        if selected {
            let fill = crate::transcript::tint(t.accent, bg, if s.focus == SettingsPane::Sections { 0.32 } else { 0.18 });
            text(
                f,
                row.x,
                row.y,
                row.width,
                vec![
                    Span::styled("▌", Style::default().fg(t.accent_line).bg(fill)),
                    Span::styled(format!(" {:<w$}", name, w = side_w as usize - 2), Style::default().fg(t.text).bg(fill).add_modifier(Modifier::BOLD)),
                ],
                bg,
            );
        } else {
            text(f, row.x, row.y, row.width, vec![Span::styled(format!("  {name}"), Style::default().fg(t.text))], bg);
        }
        out.hits.push(Hit { rect: row, target: Target::Section(i) });
    }
    // The divider between the panes.
    let div_x = x + side_w + 2;
    for y in body_y - 1..footer_y - 1 {
        text(f, div_x, y, 1, vec![Span::styled("│", Style::default().fg(t.border_subtle))], bg);
    }
    // The page.
    let (px, pw) = (div_x + 3, (inner.x + inner.width).saturating_sub(div_x + 5));
    let name = s.sections.get(s.section).cloned().unwrap_or_default();
    text(f, px, body_y, pw, vec![Span::styled(name.to_uppercase(), Style::default().fg(t.accent).add_modifier(Modifier::BOLD))], bg);
    if name == "Callbacks" {
        callbacks_page(f, Rect::new(px, body_y + 1, pw, footer_y.saturating_sub(body_y + 2)), s, t, bg, out);
    } else if s.section != 0 {
        text(f, px, body_y + 1, pw, vec![Span::styled(format!("{name} settings come with their stage."), Style::default().fg(t.text_muted))], bg);
    } else {
        provider_page(f, Rect::new(px, body_y + 1, pw, footer_y.saturating_sub(body_y + 2)), s, t, bg, out);
    }
    rule(f, inner.x, footer_y - 1, inner.width, t, bg);
    hints(
        f,
        Rect::new(inner.x, footer_y, inner.width, 1),
        &[("↑↓", "navigate"), ("Tab", "change pane"), ("Space", "select"), ("Enter", "edit"), ("Ctrl+S", "save"), ("Esc", "close")],
        t,
        bg,
    );
}

/// What the Login button says, for the chosen adapter and its state.
fn login_label(s: &SettingsView) -> &'static str {
    match (on_plan(s), s.auth.pending, s.auth.signed_in) {
        (true, true, _) => "Cancel",
        (true, false, true) => "Log out",
        (true, false, false) => "Login",
        (false, ..) if s.providers.get(s.provider).is_some_and(|p| p.name == "Local") => "No key needed",
        (false, ..) if on_delegate(s) => "Signed in by Claude Code",
        (false, ..) => "Enter a key",
    }
}

fn on_delegate(s: &SettingsView) -> bool {
    s.providers.get(s.provider).is_some_and(|p| p.name == DELEGATE_PROVIDER)
}

fn on_plan(s: &SettingsView) -> bool {
    s.providers.get(s.provider).is_some_and(|p| p.name == PLAN_PROVIDER)
}

/// Everything under the provider list that a row's sign-in needs: its state and its button.
/// Returns the first free row.
fn sign_in_rows(f: &mut Frame, r: Rect, mut y: u16, s: &SettingsView, t: &Theme, bg: Color, out: &mut Drawn) -> u16 {
    let a = &s.auth;
    let page_focus = s.focus == SettingsPane::Page;
    let provider = s.providers.get(s.provider).map_or("", |p| p.name.as_str());
    let label = |text: &str| Span::styled(format!("{text:<22}"), Style::default().fg(t.text));
    let status = if on_plan(s) { a.plan_status.clone() } else if on_delegate(s) { "Claude Code signs in by itself".to_string() } else { a.keys.iter().find(|(p, _)| p == provider).map_or("not set".to_string(), |(_, w)| format!("key {w}")) };
    text(f, r.x, y, r.width, vec![label("Sign-in"), Span::styled(status, Style::default().fg(if on_plan(s) && a.signed_in { t.success } else { t.text_muted }))], bg);
    y += 1;
    if let Some(entry) = &a.entry {
        let dots = "•".repeat(entry.chars().count());
        text(f, r.x, y, r.width, vec![label("Key"), Span::styled(format!("[ {dots}▏ ]"), Style::default().fg(t.text)), Span::styled("  Enter saves, Esc cancels", Style::default().fg(t.text_muted))], bg);
    } else {
        let under = page_focus && s.cursor == s.providers.len();
        let button = Style::default().fg(if under { t.surface } else { t.text }).bg(if under { t.accent } else { t.surface_raised }).add_modifier(Modifier::BOLD);
        text(f, r.x, y, r.width, vec![label(""), Span::styled(format!("  {}  ", login_label(s)), button)], bg);
        out.hits.push(Hit { rect: Rect::new(r.x, y, r.width, 1), target: Target::AuthButton });
    }
    y + 2
}

/// The outcome of a sign-in, wrapped: an address is long, and has to stay whole.
fn sign_in_message(f: &mut Frame, r: Rect, mut y: u16, s: &SettingsView, t: &Theme, bg: Color) {
    let width = r.width.max(1) as usize;
    for message in &s.auth.message {
        let chars: Vec<char> = message.chars().collect();
        for part in chars.chunks(width) {
            if y >= r.y + r.height {
                return;
            }
            text(f, r.x, y, r.width, vec![Span::styled(part.iter().collect::<String>(), Style::default().fg(t.text_muted))], bg);
            y += 1;
        }
    }
}

/// A key typed into the key field, which takes every key while it is open.
fn key_entry(w: &mut Workspace, k: Key) -> bool {
    let Some(Overlay::Settings(s)) = &mut w.overlay else { return false };
    let Some(entry) = &mut s.auth.entry else { return false };
    match k {
        Key::Char(c) => entry.push(c),
        Key::Backspace => {
            entry.pop();
        }
        Key::Esc => s.auth.entry = None,
        Key::Enter => {
            let key = s.auth.entry.take().unwrap_or_default();
            if !key.trim().is_empty() {
                let provider = s.providers[s.provider].name.clone();
                w.outbox.push(Outgoing::SaveKey { provider, key: harness_api::Secret::new(key.trim().to_string()) });
            }
        }
        _ => {}
    }
    true
}

/// The sign-in button runs the pipeline of the row chosen.
fn press_sign_in(w: &mut Workspace) {
    let Some(Overlay::Settings(s)) = &mut w.overlay else { return };
    match (on_plan(s), s.auth.pending, s.auth.signed_in) {
        (true, true, _) => w.outbox.push(Outgoing::LoginCancel),
        (true, false, true) => w.outbox.push(Outgoing::Logout),
        (true, false, false) => w.outbox.push(Outgoing::Login),
        (false, ..) if s.providers[s.provider].name == "Local" => s.auth.message = vec!["A local model needs no key.".to_string()],
        (false, ..) if on_delegate(s) => s.auth.message = vec!["The harness never sees this sign-in. To change it, run claude in a terminal and use /login there.".to_string()],
        (false, ..) => s.auth.entry = Some(String::new()),
    }
}

/// The model row: the name, or what it takes to get one.
fn model_value(s: &SettingsView, t: &Theme) -> Vec<Span<'static>> {
    let (muted, strong) = (Style::default().fg(t.text_muted), Style::default().fg(t.text).add_modifier(Modifier::BOLD));
    if s.editing_model {
        return vec![Span::styled("[ ", muted), Span::styled(format!("{}▏", s.model), strong), Span::styled(" ]", muted)];
    }
    if s.model.is_empty() {
        let hint = if on_plan(s) { "sign in to see this plan's models" } else { "Enter types a model's name" };
        return vec![Span::styled(format!("[ {hint} ]"), muted)];
    }
    let detail = models_for(s).into_iter().find(|m| m.id == s.model).map_or(String::new(), |m| m.detail);
    let more = if detail.is_empty() { String::new() } else { format!("  {detail}") };
    vec![Span::styled("[ ", muted), Span::styled(s.model.clone(), strong), Span::styled(" ▾ ]", muted), Span::styled(more, muted)]
}

/// The models the chosen provider offers, as far as is known.
fn models_for(s: &SettingsView) -> Vec<ModelChoice> {
    let row = s.providers.get(s.provider).map_or("", |p| p.name.as_str());
    if row == PLAN_PROVIDER {
        return s.auth.models.clone();
    }
    let listed = s.auth.provider_models.iter().find(|(p, _)| p == row).map_or(Vec::new(), |(_, m)| m.clone());
    // Anthropic's list needs a key too; until there is one, the models the harness knows.
    if listed.is_empty() && (row == "Anthropic API" || row == DELEGATE_PROVIDER) {
        return MODELS.iter().map(|m| ModelChoice { id: m.to_string(), detail: String::new(), context: None }).collect();
    }
    listed
}

/// Keeps the model one the chosen provider has: its first, or none until a list or a name comes.
/// A name typed for a provider with no list yet stays.
pub fn fit_model(s: &mut SettingsView) {
    let list = models_for(s);
    if list.iter().any(|m| m.id == s.model) {
        return;
    }
    let has = |known: &[ModelChoice]| known.iter().any(|m| m.id == s.model);
    let elsewhere = MODELS.iter().any(|m| *m == s.model) || has(&s.auth.models) || s.auth.provider_models.iter().any(|(_, m)| has(m));
    match list.first() {
        Some(first) => s.model = first.id.clone(),
        None if elsewhere || on_plan(s) => s.model.clear(),
        None => {}
    }
}

/// Why the adapter and model cannot be changed right now (CON-79).
const RUN_ON: &str = "A run is on: the adapter and model can be changed when it ends.";

/// A provider row chosen. A disabled one answers with a toast and changes nothing.
fn choose_provider(w: &mut Workspace, i: usize) {
    let Some(Overlay::Settings(s)) = &mut w.overlay else { return };
    if s.run_on {
        w.toast = Some(system_toast(RUN_ON));
        return;
    }
    if s.providers[i].disabled.is_some() {
        w.toast = Some(system_toast(harness_api::CLAUDE_SUBSCRIPTION_REFUSED));
        return;
    }
    s.provider = i;
    fit_model(s);
    s.context = chosen_context(s);
}

fn provider_page(f: &mut Frame, r: Rect, s: &SettingsView, t: &Theme, bg: Color, out: &mut Drawn) {
    // A sign-in under way has the page to itself: its address has to fit.
    if s.auth.pending {
        text(f, r.x, r.y, r.width, vec![Span::styled("Signing in to your ChatGPT plan.", Style::default().fg(t.text_muted))], bg);
        let y = sign_in_rows(f, r, r.y + 2, s, t, bg, out);
        sign_in_message(f, r, y, s, t, bg);
        return;
    }
    text(f, r.x, r.y, r.width, vec![Span::styled("Select the model adapter and defaults.", Style::default().fg(t.text_muted))], bg);
    let shipped = s.providers.iter().filter(|p| !p.local_build).count();
    let rows = s.providers.len() as u16 + u16::from(shipped < s.providers.len());
    let group = modal(f, Rect::new(r.x, r.y + 2, r.width, rows + 2), t.border_subtle, bg);
    let page_focus = s.focus == SettingsPane::Page;
    let mut y = group.y;
    for (i, p) in s.providers.iter().enumerate() {
        if i == shipped {
            text(f, group.x + 1, y, group.width - 2, vec![Span::styled("─".repeat(group.width as usize - 2), Style::default().fg(t.border_subtle))], bg);
            y += 1;
        }
        let under = page_focus && s.cursor == i;
        let row_bg = if under { t.surface_raised } else { bg };
        let radio = if s.provider == i {
            vec![Span::styled(" ( ", Style::default().fg(t.text)), Span::styled("●", Style::default().fg(t.success)), Span::styled(" )  ", Style::default().fg(t.text))]
        } else {
            vec![Span::styled(" (   )  ", Style::default().fg(t.text_muted))]
        };
        let mut spans = radio;
        spans.push(Span::styled(p.name.clone(), Style::default().fg(t.text).add_modifier(if s.provider == i { Modifier::BOLD } else { Modifier::empty() })));
        // The description only for the row under the cursor, where there is room for it.
        if under && !p.local_build && p.disabled.is_none() {
            spans.push(Span::styled(format!("   {}", p.description), Style::default().fg(t.text_muted)));
        }
        let row = Rect::new(group.x, y, group.width, 1);
        if let Some(reason) = &p.disabled {
            // Dim, always with its reason, and no radio to fill.
            let dim = Style::default().fg(t.text_muted);
            spans = vec![Span::styled(" (   )  ", dim), Span::styled(p.name.clone(), dim), Span::styled(format!("   {reason}"), dim)];
        } else if s.run_on {
            // Dim, keeping which one it is on, and why only under the cursor.
            let dim = Style::default().fg(t.text_muted);
            let radio = if s.provider == i { " ( ● )  " } else { " (   )  " };
            spans = vec![Span::styled(radio, dim), Span::styled(p.name.clone(), dim)];
            if under {
                spans.push(Span::styled(format!("   {RUN_ON}"), dim));
            }
        }
        text(f, row.x, row.y, row.width, spans, row_bg);
        if p.local_build {
            let pill = crate::transcript::pill_spans("local build", t.text, t.surface_raised, row_bg);
            let pw: u16 = 13;
            f.render_widget(Paragraph::new(Line::from(pill)), Rect::new(row.x + row.width - pw - 1, y, pw, 1));
        }
        out.hits.push(Hit { rect: row, target: Target::Provider(i) });
        y += 1;
    }
    // The chosen row's sign-in, then its fields.
    let mut y = sign_in_rows(f, r, group.y + group.height + 1, s, t, bg, out);
    let model_y = y;
    let n = s.providers.len() + 1;
    let field = |label: &str, value: Vec<Span<'static>>, idx: usize| {
        let under = page_focus && s.cursor == n + idx;
        let mut spans = vec![Span::styled(format!("{label:<22}"), Style::default().fg(t.text))];
        spans.extend(value);
        (spans, if under { t.surface_raised } else { bg })
    };
    let bracket = |v: String, arrow: bool| {
        vec![
            Span::styled("[ ", Style::default().fg(t.text_muted)),
            Span::styled(v, Style::default().fg(t.text).add_modifier(Modifier::BOLD)),
            Span::styled(if arrow { " ▾ ]" } else { " ]" }, Style::default().fg(t.text_muted)),
        ]
    };
    let check = if s.fallback {
        vec![Span::styled("[", Style::default().fg(t.text_muted)), Span::styled("x", Style::default().fg(t.success).add_modifier(Modifier::BOLD)), Span::styled("]  Enabled", Style::default().fg(t.text))]
    } else {
        vec![Span::styled("[ ]  Disabled", Style::default().fg(t.text_muted))]
    };
    for (spans, row_bg) in [
        field("Default model", model_value(s, t), 0),
        field("Context window", bracket(context_label(s.context), true), 1),
        field("Timeout", bracket(format!("{}s", s.timeout_s), false), 2),
        field("Fallback on error", check, 3),
    ] {
        if y >= r.y + r.height {
            break;
        }
        text(f, r.x, y, r.width, spans, row_bg);
        y += 1;
    }
    out.hits.push(Hit { rect: Rect::new(r.x, model_y, r.width, 1), target: Target::ModelRow });
    out.hits.push(Hit { rect: Rect::new(r.x, model_y + 1, r.width, 1), target: Target::ContextRow });
    if on_delegate(s) {
        let n = s.providers.len() + 1;
        let tick = |on: bool, words: &str| -> Vec<Span<'static>> {
            vec![Span::styled(if on { "[x]" } else { "[ ]" }, Style::default().fg(if on { t.success } else { t.text_muted })), Span::styled(format!("  {words}"), Style::default().fg(if on { t.text } else { t.text_muted }))]
        };
        for (i, (label, spans, target)) in [
            ("Your environment", tick(s.delegate_own, "your hooks, skills, servers and browser"), Target::DelegateOwn),
            ("No permission prompts", tick(s.delegate_skip, "it may do anything, as you can yourself"), Target::DelegateSkip),
        ].into_iter().enumerate() {
            if y >= r.y + r.height {
                break;
            }
            let under = page_focus && s.cursor == n + 4 + i;
            let mut row = vec![Span::styled(format!("{label:<22}"), Style::default().fg(t.text))];
            row.extend(spans);
            text(f, r.x, y, r.width, row, if under { t.surface_raised } else { bg });
            out.hits.push(Hit { rect: Rect::new(r.x, y, r.width, 1), target });
            y += 1;
        }
    }
    sign_in_message(f, r, y + 1, s, t, bg);
    if let Some(at) = s.context_menu {
        context_dropdown(f, r, model_y + 1, at, s, t, out);
    }
    if let Some(at) = s.model_menu {
        model_dropdown(f, r, model_y, at, s, t, out);
    }
    if let Some(at) = s.context_menu {
        context_dropdown(f, r, model_y + 1, at, s, t, out);
    }
}

/// The open list of context windows, under its row.
fn context_dropdown(f: &mut Frame, r: Rect, row_y: u16, at: usize, s: &SettingsView, t: &Theme, out: &mut Drawn) {
    let shown = (CONTEXT_WINDOWS.len() as u16).min(r.height.saturating_sub(2).max(1));
    let top = (row_y + 1).min((r.y + r.height).saturating_sub(shown + 2));
    let inner = modal(f, Rect::new(r.x, top, r.width, shown + 2), t.accent_line, t.surface);
    let first = at.saturating_sub(shown as usize - 1);
    for (row, (i, window)) in CONTEXT_WINDOWS.iter().enumerate().skip(first).take(shown as usize).enumerate() {
        let y = inner.y + row as u16;
        let under = i == at;
        let row_bg = if under { t.surface_raised } else { t.surface };
        let current = if *window == s.context { "● " } else { "  " };
        let detail = if window.is_none() { "what the server or the model says" } else { "" };
        let spans = vec![
            Span::styled(format!(" {current}"), Style::default().fg(t.success)),
            Span::styled(format!("{:<8}", context_label(*window)), Style::default().fg(t.text).add_modifier(if under { Modifier::BOLD } else { Modifier::empty() })),
            Span::styled(detail, Style::default().fg(t.text_muted)),
        ];
        let rect = Rect::new(inner.x, y, inner.width, 1);
        text(f, rect.x, rect.y, rect.width, spans, row_bg);
        out.hits.push(Hit { rect, target: Target::ContextChoice(i) });
    }
}

/// The open list of models, under the model row, each with what it costs.
fn model_dropdown(f: &mut Frame, r: Rect, row_y: u16, at: usize, s: &SettingsView, t: &Theme, out: &mut Drawn) {
    let list = models_for(s);
    let shown = (list.len() as u16).min(r.height.saturating_sub(2).max(1)).min(10);
    let h = shown + 2;
    let top = (row_y + 1).min((r.y + r.height).saturating_sub(h));
    let inner = modal(f, Rect::new(r.x, top, r.width, h), t.accent_line, t.surface);
    let first = at.saturating_sub(shown as usize - 1);
    let id_w = list.iter().map(|m| m.id.chars().count()).max().unwrap_or(0) + 2;
    for (row, (i, choice)) in list.iter().enumerate().skip(first).take(shown as usize).enumerate() {
        let y = inner.y + row as u16;
        let under = i == at;
        let row_bg = if under { t.surface_raised } else { t.surface };
        let current = if choice.id == s.model { "● " } else { "  " };
        let spans = vec![
            Span::styled(format!(" {current}"), Style::default().fg(t.success)),
            Span::styled(format!("{:<id_w$}", choice.id), Style::default().fg(t.text).add_modifier(if under { Modifier::BOLD } else { Modifier::empty() })),
            Span::styled(choice.detail.clone(), Style::default().fg(t.text_muted)),
        ];
        let rect = Rect::new(inner.x, y, inner.width, 1);
        text(f, rect.x, rect.y, rect.width, spans, row_bg);
        out.hits.push(Hit { rect, target: Target::ModelChoice(i) });
    }
}

/// What a key does while an overlay is open. Returns None when no overlay is open.
pub fn key(w: &mut Workspace, k: Key) -> Option<Outcome> {
    if let Some(Overlay::Settings(s)) = &mut w.overlay {
        if s.editing_model {
            match k {
                Key::Char(c) => s.model.push(c),
                Key::Backspace => {
                    s.model.pop();
                }
                Key::Enter | Key::Esc => s.editing_model = false,
                _ => {}
            }
            return Some(Outcome::Stay);
        }
        if let Some(at) = s.model_menu {
            let list = models_for(s);
            match k {
                Key::Up => s.model_menu = Some(at.saturating_sub(1)),
                Key::Down => s.model_menu = Some((at + 1).min(list.len().saturating_sub(1))),
                Key::Enter | Key::Char(' ') => {
                    if let Some(choice) = list.get(at) {
                        s.model = choice.id.clone();
                    }
                    s.model_menu = None;
                }
                Key::Esc => s.model_menu = None,
                _ => {}
            }
            return Some(Outcome::Stay);
        }
        if let Some(at) = s.context_menu {
            match k {
                Key::Up => s.context_menu = Some(at.saturating_sub(1)),
                Key::Down => s.context_menu = Some((at + 1).min(CONTEXT_WINDOWS.len() - 1)),
                Key::Enter | Key::Char(' ') => {
                    s.context = CONTEXT_WINDOWS[at];
                    s.context_menu = None;
                }
                Key::Esc => s.context_menu = None,
                _ => {}
            }
            return Some(Outcome::Stay);
        }
        if key_entry(w, k) {
            return Some(Outcome::Stay);
        }
    }
    let overlay = w.overlay.as_mut()?;
    match overlay {
        Overlay::Syncing(_) => {}
        Overlay::Permission(p) => {
            let chosen = match k {
                Key::Left | Key::Up => {
                    p.choice = (p.choice + 2) % 3;
                    None
                }
                Key::Right | Key::Down | Key::Tab => {
                    p.choice = (p.choice + 1) % 3;
                    None
                }
                Key::Char(c @ '1'..='3') => Some(c as usize - '1' as usize),
                Key::Enter => Some(p.choice),
                Key::Esc => Some(2),
                _ => None,
            };
            if let Some(i) = chosen {
                answer(w, i);
            }
        }
        Overlay::Settings(s) if s.sections.get(s.section).is_some_and(|n| n == "Callbacks") && !matches!(k, Key::Esc) && (s.focus == SettingsPane::Page || matches!(k, Key::Ctrl('s'))) => {
            let p = &mut s.callbacks;
            match k {
                Key::Tab | Key::Left | Key::Right => s.focus = SettingsPane::Sections,
                // An open dropdown takes the arrows; it closes on Space or Enter.
                Key::Up | Key::Down if p.open.is_some() => {
                    let row = &mut p.standard[p.open.unwrap_or(0)];
                    let n = row.implementations.len();
                    row.chosen = if k == Key::Up { (row.chosen + n - 1) % n } else { (row.chosen + 1) % n };
                }
                Key::Up => p.cursor = p.cursor.saturating_sub(1),
                Key::Down => p.cursor = (p.cursor + 1).min(p.rows().saturating_sub(1)),
                Key::Char(' ') | Key::Enter => activate_callback(p),
                Key::Char('r') => w.outbox.push(Outgoing::ReloadCallbacks),
                Key::Ctrl('s') => {
                    let handlers = p.standard.iter().map(|r| (r.name.clone(), r.implementations.get(r.chosen).cloned().unwrap_or_default())).collect();
                    let disabled = p.custom.iter().filter(|c| !c.on).map(|c| c.name.clone()).chain(p.conditions.iter().filter(|c| !c.on).map(|c| c.name.clone())).collect();
                    w.overlay = None;
                    w.outbox.push(Outgoing::SaveCallbacks { handlers, disabled });
                }
                _ => {}
            }
        }
        Overlay::Settings(s) => match k {
            Key::Esc => {
                // Closing is not saving, and the choice should not go quietly.
                let lost = (s.provider, s.model.clone()) != s.saved || s.context != s.saved_context || (s.delegate_own, s.delegate_skip) != s.saved_delegate;
                w.overlay = None;
                if lost {
                    w.toast = Some(system_toast("Not saved: Ctrl+S saves the provider and model."));
                }
            }
            Key::Ctrl('s') if s.run_on => w.toast = Some(system_toast(RUN_ON)),
            Key::Enter | Key::Char(_) | Key::Backspace if s.run_on && s.focus == SettingsPane::Page => w.toast = Some(system_toast(RUN_ON)),
            Key::Ctrl('s') => {
                // Save: the session's status line shows the chosen adapter and model.
                let adapter = s.providers[s.provider].name.to_lowercase().replace(' ', "-");
                let model = s.model.clone();
                if model.trim().is_empty() {
                    w.toast = Some(system_toast("Choose a model first."));
                    return Some(Outcome::Stay);
                }
                if w.live {
                    let provider = s.providers[s.provider].name.clone();
                    let context = s.context;
                    let delegate = (s.delegate_own, s.delegate_skip);
                    w.overlay = None;
                    w.outbox.push(Outgoing::SaveSettings { provider, model, context, delegate });
                    return Some(Outcome::Stay);
                }
                w.overlay = None;
                for (k, v) in &mut w.view_mut().status.fields {
                    match k.as_str() {
                        "ADAPTER" => *v = adapter.clone(),
                        "MODEL" => *v = model.clone(),
                        _ => {}
                    }
                }
            }
            Key::Tab | Key::Left | Key::Right => {
                s.focus = if s.focus == SettingsPane::Sections { SettingsPane::Page } else { SettingsPane::Sections };
            }
            Key::Up => match s.focus {
                SettingsPane::Sections => s.section = s.section.saturating_sub(1),
                SettingsPane::Page => s.cursor = s.cursor.saturating_sub(1),
            },
            Key::Down => match s.focus {
                SettingsPane::Sections => s.section = (s.section + 1).min(s.sections.len() - 1),
                SettingsPane::Page => s.cursor = (s.cursor + 1).min(s.providers.len() + if on_delegate(s) { 6 } else { 4 }),
            },
            // Enter types a model's name; the list only knows a few.
            Key::Enter if s.focus == SettingsPane::Page && s.section == 0 && s.cursor == s.providers.len() + 1 && !on_plan(s) && models_for(s).is_empty() => s.editing_model = true,
            // With a list, E types a name that is not on it.
            Key::Char('e') if s.focus == SettingsPane::Page && s.section == 0 && s.cursor == s.providers.len() + 1 && !on_plan(s) => s.editing_model = true,
            Key::Char(' ') | Key::Enter if s.focus == SettingsPane::Page => activate(w),
            Key::Enter => s.focus = SettingsPane::Page,
            _ => {}
        },
    }
    Some(Outcome::Stay)
}

/// Space / Enter on the page row under the cursor: pick a provider, cycle a field, toggle a box.
fn activate(w: &mut Workspace) {
    let Some(Overlay::Settings(s)) = &mut w.overlay else { return };
    let n = s.providers.len();
    match s.cursor {
        i if i < n => choose_provider(w, i),
        i if i == n => press_sign_in(w),
        i if i == n + 1 => {
            let list = models_for(s);
            if !list.is_empty() {
                s.model_menu = Some(list.iter().position(|m| m.id == s.model).unwrap_or(0));
            }
        }
        i if i == n + 2 => s.context_menu = Some(CONTEXT_WINDOWS.iter().position(|w| *w == s.context).unwrap_or(0)),
        i if i == n + 3 => {
            let at = TIMEOUTS.iter().position(|v| *v == s.timeout_s).unwrap_or(0);
            s.timeout_s = TIMEOUTS[(at + 1) % TIMEOUTS.len()];
        }
        i if i == n + 4 => s.fallback = !s.fallback,
        i if i == n + 5 && on_delegate(s) => s.delegate_own = !s.delegate_own,
        i if i == n + 6 && on_delegate(s) => s.delegate_skip = !s.delegate_skip,
        _ => {}
    }
}

/// The user answered the permission prompt: the call shows in the transcript with the answer.
pub fn answer(w: &mut Workspace, choice: usize) {
    let Some(Overlay::Permission(p)) = w.overlay.take() else { return };
    if p.locked && choice != 2 {
        w.overlay = Some(Overlay::Permission(p));
        return;
    }
    // A running session draws the call itself, from its log.
    if let Some(tab) = p.tab {
        w.outbox.push(Outgoing::Answer { tab, choice });
        return;
    }
    let (state, outcome, tone) = match choice {
        0 => (ToolState::Done, "allowed once", Tone::Success),
        1 => (ToolState::Done, "always allowed · rule saved", Tone::Success),
        _ => (ToolState::Failed, "denied", Tone::Error),
    };
    let call = ToolCall {
        name: p.tool.clone(),
        args: p.command.clone(),
        args_strong: None,
        state,
        outcome: outcome.into(),
        outcome_tone: tone,
        diffstat: None,
        duration: String::new(),
        expanded: false,
        body: ToolBody::None,
        icon: ToolIcon::None,
        spin_ms: 0,
    };
    let v = w.view_mut();
    match v.transcript.last_mut() {
        Some(Item::Turn { role: Role::Assistant, attached, .. }) => attached.push(Item::Tool(call)),
        _ => v.transcript.push(Item::Tool(call)),
    }
}

/// A click inside an open overlay.
pub fn click(w: &mut Workspace, target: Target) -> Outcome {
    match (&mut w.overlay, target) {
        (Some(Overlay::Permission(_)), Target::Choice(i)) => answer(w, i),
        (Some(Overlay::Settings(s)), Target::Section(i)) => {
            s.section = i;
            s.focus = SettingsPane::Sections;
        }
        (Some(Overlay::Settings(s)), Target::CallbackRow(i)) => {
            s.focus = SettingsPane::Page;
            s.callbacks.cursor = i;
            activate_callback(&mut s.callbacks);
        }
        (Some(Overlay::Settings(s)), Target::Provider(i)) => {
            s.cursor = i;
            s.focus = SettingsPane::Page;
            choose_provider(w, i);
        }
        (Some(Overlay::Settings(s)), Target::ModelRow | Target::ModelChoice(_) | Target::AuthButton | Target::ContextRow | Target::ContextChoice(_) | Target::DelegateOwn | Target::DelegateSkip) if s.run_on => w.toast = Some(system_toast(RUN_ON)),
        (Some(Overlay::Settings(s)), Target::DelegateOwn) => s.delegate_own = !s.delegate_own,
        (Some(Overlay::Settings(s)), Target::DelegateSkip) => s.delegate_skip = !s.delegate_skip,
        (Some(Overlay::Settings(s)), Target::ContextRow) => {
            s.cursor = s.providers.len() + 2;
            s.focus = SettingsPane::Page;
            activate(w);
        }
        (Some(Overlay::Settings(s)), Target::ContextChoice(i)) => {
            s.context = CONTEXT_WINDOWS[i];
            s.context_menu = None;
        }
        (Some(Overlay::Settings(s)), Target::ModelRow) => {
            s.cursor = s.providers.len() + 1;
            s.focus = SettingsPane::Page;
            activate(w);
        }
        (Some(Overlay::Settings(s)), Target::ModelChoice(i)) => {
            if let Some(choice) = models_for(s).get(i) {
                s.model = choice.id.clone();
            }
            s.model_menu = None;
        }
        (Some(Overlay::Settings(s)), Target::AuthButton) => {
            s.cursor = s.providers.len();
            s.focus = SettingsPane::Page;
            activate(w);
        }
        (Some(_), Target::CloseOverlay) => w.overlay = None,
        _ => {}
    }
    Outcome::Stay
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pick(id: &str) -> ModelChoice {
        ModelChoice { id: id.into(), detail: format!("{id} costs"), context: None }
    }

    #[test]
    fn a_windows_outline_shows_nothing_of_what_is_under_it() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut terminal = Terminal::new(TestBackend::new(20, 8)).unwrap();
        let (under, bg, line) = (Color::Red, Color::Blue, Color::Green);
        terminal
            .draw(|f| {
                let all = f.area();
                let buf = f.buffer_mut();
                for y in 0..all.height {
                    for x in 0..all.width {
                        buf[(x, y)].set_symbol("▆").set_fg(Color::Yellow).set_bg(under);
                    }
                }
                modal(f, Rect::new(2, 1, 16, 6), line, bg);
            })
            .unwrap();
        let buf = terminal.backend().buffer();
        for y in 1..7 {
            for x in 2..18 {
                assert_eq!(buf[(x, y)].bg, bg, "({x},{y})");
                assert_ne!(buf[(x, y)].symbol(), "▆", "({x},{y})");
            }
        }
        assert_eq!(buf[(1u16, 1u16)].bg, under, "outside is left alone");
    }

    #[test]
    fn closing_with_a_change_unsaved_says_so_and_with_none_does_not() {
        let mut w = on_provider_page();
        press(&mut w, &[Key::Esc]);
        assert!(w.overlay.is_none() && w.toast.is_none());
        let mut w = on_provider_page();
        let plan = row(&w, PLAN_PROVIDER);
        to(&mut w, plan);
        press(&mut w, &[Key::Enter, Key::Esc]);
        assert!(w.overlay.is_none());
        assert!(matches!(&w.toast, Some(t) if t.text.contains("Ctrl+S")));
    }

    #[test]
    fn settings_open_on_what_the_status_line_says_and_drop_old_news() {
        let mut w = on_provider_page();
        w.overlay = None;
        w.view_mut().status.fields = vec![("ADAPTER".into(), "chatgpt".into()), ("MODEL".into(), "gpt-x".into())];
        w.auth = Some(AuthPage { message: vec!["old".into()], ..Default::default() });
        let s = settings_for(&w);
        assert_eq!((s.providers[s.provider].name.as_str(), s.model.as_str()), (PLAN_PROVIDER, "gpt-x"));
        assert!(s.auth.message.is_empty());
        assert_eq!(s.saved, (s.provider, "gpt-x".to_string()));
    }

    fn on_provider_page() -> Workspace {
        let mut w = match crate::fixtures::welcome().mode {
            Mode::Session(w) => w,
            _ => unreachable!(),
        };
        w.live = true;
        let mut s = sample_settings();
        s.focus = SettingsPane::Page;
        s.auth = AuthPage { plan_status: "Not signed in".into(), ..sample_auth() };
        s.auth.signed_in = false;
        w.overlay = Some(Overlay::Settings(s));
        w
    }

    fn page(w: &Workspace) -> &SettingsView {
        match &w.overlay {
            Some(Overlay::Settings(s)) => s,
            _ => panic!("no settings"),
        }
    }

    fn press(w: &mut Workspace, keys: &[Key]) {
        for k in keys {
            assert!(key(w, *k).is_some());
        }
    }

    fn row(w: &Workspace, name: &str) -> usize {
        page(w).providers.iter().position(|p| p.name == name).unwrap()
    }

    fn to(w: &mut Workspace, cursor: usize) {
        if let Some(Overlay::Settings(s)) = &mut w.overlay {
            s.cursor = cursor;
        }
    }

    #[test]
    fn the_delegates_row_shows_only_in_a_build_that_has_it() {
        let mut w = on_provider_page();
        w.overlay = None;
        assert!(!settings_for(&w).providers.iter().any(|p| p.name == DELEGATE_PROVIDER));
        w.auth = Some(AuthPage { delegate: true, ..Default::default() });
        w.view_mut().status.fields = vec![("ADAPTER".into(), "claude-code".into()), ("MODEL".into(), "claude-haiku-5-5".into())];
        let s = settings_for(&w);
        assert_eq!(s.providers[s.provider].name, DELEGATE_PROVIDER);
        assert_eq!(login_label(&s), "Signed in by Claude Code");
        assert!(models_for(&s).iter().any(|m| m.id == "claude-opus-5-5"));
    }

    #[test]
    fn one_page_holds_the_provider_and_its_sign_in() {
        let sections = sample_settings().sections;
        assert!(sections.iter().any(|n| n == "Provider"));
        assert!(!sections.iter().any(|n| n == "Auth" || n == "Keys"), "{sections:?}");
    }

    #[test]
    fn choosing_a_row_chooses_its_sign_in_and_a_fitting_model() {
        let mut w = on_provider_page();
        assert!(!on_plan(page(&w)));
        assert_eq!(login_label(page(&w)), "Enter a key");
        let plan = row(&w, PLAN_PROVIDER);
        to(&mut w, plan);
        press(&mut w, &[Key::Enter]);
        assert!(on_plan(page(&w)));
        assert_eq!(page(&w).model, "gpt-5.5", "a Claude model means nothing on this plan");
        assert_eq!(login_label(page(&w)), "Login");
        to(&mut w, 0);
        press(&mut w, &[Key::Enter]);
        assert!(!on_plan(page(&w)));
        assert!(page(&w).model.starts_with("claude"), "{}", page(&w).model);
        let local = row(&w, "Local");
        to(&mut w, local);
        press(&mut w, &[Key::Enter]);
        assert_eq!(login_label(page(&w)), "No key needed");
    }

    #[test]
    fn each_provider_has_its_own_models_and_none_keeps_anothers() {
        let mut w = on_provider_page();
        if let Some(Overlay::Settings(s)) = &mut w.overlay {
            s.auth.models.clear();
            s.auth.provider_models = vec![("Local".into(), vec![pick("qwen"), pick("gemma")])];
        }
        assert!(page(&w).model.starts_with("claude"));
        let (plan, local, openai, model) = (row(&w, PLAN_PROVIDER), row(&w, "Local"), row(&w, "OpenAI-compatible"), page(&w).providers.len() + 1);

        // Before sign-in the plan has no list: no Claude name stays, and it cannot be saved.
        to(&mut w, plan);
        press(&mut w, &[Key::Enter]);
        assert_eq!(page(&w).model, "");
        press(&mut w, &[Key::Ctrl('s')]);
        assert!(w.overlay.is_some() && w.outbox.is_empty(), "{:?}", w.outbox);
        assert!(matches!(&w.toast, Some(t) if t.text.contains("Choose a model")));
        to(&mut w, model);
        press(&mut w, &[Key::Enter, Key::Char(' ')]);
        assert_eq!((page(&w).model.as_str(), page(&w).editing_model), ("", false), "no typing a name on a plan");

        // A list that arrives is taken up.
        if let Some(Overlay::Settings(s)) = &mut w.overlay {
            s.auth.models = vec![pick("gpt-a"), pick("gpt-b")];
            fit_model(s);
        }
        assert_eq!(page(&w).model, "gpt-a");
        press(&mut w, &[Key::Enter]);
        assert_eq!(page(&w).model_menu, Some(0), "a dropdown opens on the current one");
        press(&mut w, &[Key::Down, Key::Enter]);
        assert_eq!((page(&w).model.as_str(), page(&w).model_menu), ("gpt-b", None));

        to(&mut w, local);
        press(&mut w, &[Key::Enter]);
        assert_eq!(page(&w).model, "qwen");
        to(&mut w, model);
        press(&mut w, &[Key::Enter, Key::Down, Key::Down, Key::Enter]);
        assert_eq!(page(&w).model, "gemma", "the list of this provider, not Claude's");
        press(&mut w, &[Key::Enter, Key::Up, Key::Esc]);
        assert_eq!((page(&w).model.as_str(), page(&w).model_menu), ("gemma", None), "Esc leaves the choice, and the window open");
        assert!(w.overlay.is_some());

        // No list yet and another's name: empty, and a name can be typed.
        to(&mut w, openai);
        press(&mut w, &[Key::Enter]);
        assert_eq!(page(&w).model, "");
        to(&mut w, model);
        press(&mut w, &[Key::Enter, Key::Char('m'), Key::Enter]);
        assert_eq!(page(&w).model, "m");

        to(&mut w, 0);
        press(&mut w, &[Key::Enter]);
        assert!(page(&w).model.starts_with("claude"));
    }

    #[test]
    fn the_button_runs_the_chosen_rows_pipeline() {
        let mut w = on_provider_page();
        let plan = row(&w, PLAN_PROVIDER);
        let button = page(&w).providers.len();
        to(&mut w, plan);
        press(&mut w, &[Key::Enter]);
        to(&mut w, button);
        press(&mut w, &[Key::Enter]);
        assert!(matches!(w.outbox.last(), Some(Outgoing::Login)), "{:?}", w.outbox);
        if let Some(Overlay::Settings(s)) = &mut w.overlay {
            s.auth.pending = true;
            assert_eq!(login_label(s), "Cancel");
        }
        press(&mut w, &[Key::Enter]);
        assert!(matches!(w.outbox.last(), Some(Outgoing::LoginCancel)));
        if let Some(Overlay::Settings(s)) = &mut w.overlay {
            s.auth.pending = false;
            s.auth.signed_in = true;
            assert_eq!(login_label(s), "Log out");
        }
        press(&mut w, &[Key::Enter]);
        assert!(matches!(w.outbox.last(), Some(Outgoing::Logout)));
        click(&mut w, Target::AuthButton);
        assert_eq!(w.outbox.iter().filter(|o| matches!(o, Outgoing::Logout)).count(), 2, "the mouse too");
    }

    #[test]
    fn a_key_is_typed_masked_and_never_printed() {
        let mut w = on_provider_page();
        let (openai, button) = (row(&w, "OpenAI-compatible"), page(&w).providers.len());
        to(&mut w, openai);
        press(&mut w, &[Key::Enter]);
        to(&mut w, button);
        press(&mut w, &[Key::Enter, Key::Char('s'), Key::Char('k'), Key::Char('x'), Key::Backspace, Key::Char('9')]);
        assert_eq!(page(&w).auth.entry.as_deref(), Some("sk9"));
        let printed = format!("{:?} {:?}", page(&w).auth, w.outbox);
        assert!(!printed.contains("sk9"), "{printed}");
        press(&mut w, &[Key::Enter]);
        let Some(Outgoing::SaveKey { provider, key: secret }) = w.outbox.last() else { panic!("{:?}", w.outbox) };
        assert_eq!((provider.as_str(), secret.expose()), ("OpenAI-compatible", "sk9"));
        assert!(!format!("{:?}", w.outbox).contains("sk9"), "the outbox hides it");
        assert!(page(&w).auth.entry.is_none());

        press(&mut w, &[Key::Enter, Key::Char('a'), Key::Esc]);
        assert!(page(&w).auth.entry.is_none() && w.overlay.is_some(), "Esc drops the key and keeps the window");
        assert_eq!(w.outbox.len(), 1);
    }

    #[test]
    fn a_models_name_can_be_typed_and_on_a_plan_is_chosen_from_its_list() {
        let mut w = on_provider_page();
        let model = page(&w).providers.len() + 1;
        to(&mut w, model);
        if let Some(Overlay::Settings(s)) = &mut w.overlay {
            s.model = "x".into();
        }
        press(&mut w, &[Key::Char('e'), Key::Backspace, Key::Char('g'), Key::Char(' '), Key::Enter]);
        assert_eq!((page(&w).model.as_str(), page(&w).editing_model), ("g ", false));

        let plan = row(&w, PLAN_PROVIDER);
        to(&mut w, plan);
        press(&mut w, &[Key::Enter]);
        to(&mut w, model);
        press(&mut w, &[Key::Enter, Key::Down, Key::Enter]);
        assert_eq!(page(&w).model, "gpt-5.5-mini");
    }

    #[test]
    fn the_claude_subscription_option_shows_and_cannot_be_chosen() {
        let mut w = on_provider_page();
        assert!(!page(&w).providers.iter().any(|p| p.name.contains("acme")));
        let claude = row(&w, CLAUDE_SUBSCRIPTION);
        let before = page(&w).provider;
        assert!(page(&w).providers[claude].disabled.is_some());
        to(&mut w, claude);
        press(&mut w, &[Key::Enter]);
        assert_eq!(page(&w).provider, before, "by key");
        assert!(matches!(&w.toast, Some(Toast { kind: InboundKind::System, text, .. }) if text.contains("forbidden by Anthropic")));
        w.toast = None;
        click(&mut w, Target::Provider(claude));
        assert_eq!(page(&w).provider, before, "by click");
        assert!(w.toast.is_some() && w.outbox.is_empty(), "{:?}", w.outbox);
    }

    #[test]
    fn a_locked_prompt_answers_only_deny() {
        let mut w = match crate::fixtures::welcome().mode {
            Mode::Session(w) => w,
            _ => unreachable!(),
        };
        let open = |w: &mut Workspace| w.overlay = Some(Overlay::Permission(PermissionView { locked: true, tab: Some(1), ..sample_permission() }));
        open(&mut w);
        answer(&mut w, 0);
        answer(&mut w, 1);
        assert!(matches!(w.overlay, Some(Overlay::Permission(_))) && w.outbox.is_empty(), "{:?}", w.outbox);
        answer(&mut w, 2);
        assert!(w.overlay.is_none());
        assert!(matches!(w.outbox.last(), Some(Outgoing::Answer { choice: 2, .. })), "{:?}", w.outbox);

        open(&mut w);
        if let Some(Overlay::Permission(p)) = &mut w.overlay {
            p.locked = false;
        }
        answer(&mut w, 0);
        assert!(matches!(w.outbox.last(), Some(Outgoing::Answer { choice: 0, .. })), "{:?}", w.outbox);
    }
    use crate::model::Mode;

    fn workspace() -> Workspace {
        match crate::fixtures::main_session().mode {
            Mode::Session(w) => w,
            _ => unreachable!(),
        }
    }

    #[test]
    fn the_callbacks_tab_toggles_chooses_and_saves() {
        let mut w = workspace();
        w.live = true;
        let mut s = sample_settings();
        s.section = s.sections.iter().position(|n| n == "Callbacks").unwrap();
        s.focus = SettingsPane::Page;
        s.callbacks = sample_callbacks();
        w.overlay = Some(Overlay::Settings(s));
        let page = |w: &Workspace| match &w.overlay {
            Some(Overlay::Settings(s)) => s.callbacks.clone(),
            _ => panic!("the window closed"),
        };
        // Standard row 1: unfold, choose the next implementation, fold.
        key(&mut w, Key::Down);
        key(&mut w, Key::Enter);
        assert_eq!(page(&w).open, Some(1));
        key(&mut w, Key::Down);
        key(&mut w, Key::Enter);
        assert_eq!((page(&w).open, page(&w).standard[1].chosen), (None, 1));
        // Down to the first custom callback and the first condition: flip them.
        for _ in 0..3 {
            key(&mut w, Key::Down);
        }
        key(&mut w, Key::Char(' '));
        assert!(!page(&w).custom[0].on);
        for _ in 0..2 {
            key(&mut w, Key::Down);
        }
        key(&mut w, Key::Char(' '));
        assert!(!page(&w).conditions[0].on);
        key(&mut w, Key::Char('r'));
        assert_eq!(w.outbox.pop(), Some(Outgoing::ReloadCallbacks));
        key(&mut w, Key::Ctrl('s'));
        assert!(w.overlay.is_none());
        let Some(Outgoing::SaveCallbacks { handlers, disabled }) = w.outbox.pop() else { panic!() };
        assert!(handlers.contains(&("journal".to_string(), "off".to_string())));
        for name in ["deploy_done", "nightly_report", "pipeline keeps failing", "tests are flaky"] {
            assert!(disabled.iter().any(|d| d == name), "{name}");
        }
    }

    #[test]
    fn the_permission_prompt_answers_with_keys() {
        for (keys, outcome) in [(vec![Key::Enter], "allowed once"), (vec![Key::Right, Key::Enter], "always allowed · rule saved"), (vec![Key::Char('3')], "denied"), (vec![Key::Esc], "denied")] {
            let mut w = workspace();
            w.overlay = Some(Overlay::Permission(sample_permission()));
            for k in keys {
                key(&mut w, k);
            }
            assert!(w.overlay.is_none(), "{outcome}: the prompt closes");
            let shown = w.view().transcript.iter().any(|i| match i {
                Item::Tool(c) => c.outcome == outcome,
                Item::Turn { attached, .. } => attached.iter().any(|a| matches!(a, Item::Tool(c) if c.outcome == outcome)),
                _ => false,
            });
            assert!(shown, "{outcome} shows in the transcript");
        }
    }

    #[test]
    fn settings_move_between_panes_and_pick_a_provider() {
        let mut w = workspace();
        w.overlay = Some(Overlay::Settings(sample_settings()));
        key(&mut w, Key::Tab);
        key(&mut w, Key::Down);
        key(&mut w, Key::Char(' '));
        match &w.overlay {
            Some(Overlay::Settings(s)) => assert_eq!((s.focus, s.provider), (SettingsPane::Page, 1)),
            _ => panic!("settings stay open"),
        }
        if let Some(Overlay::Settings(s)) = &mut w.overlay {
            s.model = "some-model".into();
        }
        key(&mut w, Key::Ctrl('s'));
        assert!(w.overlay.is_none(), "Ctrl+S saves and closes");
        assert!(w.view().status.fields.iter().any(|(k, v)| k == "ADAPTER" && v == "openai-compatible"));
    }

    /// CON-80: chosen from a dropdown, saved as chosen; locked during a run (CON-79).
    #[test]
    fn the_context_window_is_a_dropdown_and_is_locked_during_a_run() {
        let mut w = on_provider_page();
        w.live = true;
        click(&mut w, Target::ContextRow);
        assert_eq!(page(&w).context_menu, Some(0), "it opens on Auto");
        key(&mut w, Key::Down);
        key(&mut w, Key::Enter);
        assert_eq!((page(&w).context, page(&w).context_menu), (Some(32_000), None));
        if let Some(Overlay::Settings(s)) = &mut w.overlay {
            s.run_on = true;
        }
        click(&mut w, Target::ContextRow);
        assert_eq!(page(&w).context_menu, None, "not during a run");
        if let Some(Overlay::Settings(s)) = &mut w.overlay {
            s.run_on = false;
        }
        key(&mut w, Key::Ctrl('s'));
        assert!(w.outbox.iter().any(|o| matches!(o, Outgoing::SaveSettings { context: Some(32_000), .. })));
    }
}
