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
    }
}

/// The settings window, opening on what the active session's status line says.
pub fn settings_for(w: &Workspace) -> SettingsView {
    let mut s = sample_settings();
    if let Some(page) = &w.callbacks {
        s.callbacks = page.clone();
    }
    for (key, value) in &w.view().status.fields {
        match key.as_str() {
            "MODEL" => s.model = value.clone(),
            "ADAPTER" => {
                let name = match value.as_str() {
                    "anthropic" | "anthropic-api" => "Anthropic API",
                    "openai" | "openai-compatible" => "OpenAI-compatible",
                    "local" => "Local",
                    other => other,
                };
                if let Some(i) = s.providers.iter().position(|p| p.name == name) {
                    s.provider = i;
                }
            }
            _ => {}
        }
    }
    s
}

pub fn sample_settings() -> SettingsView {
    let p = |name: &str, description: &str, local_build: bool| ProviderOption { name: name.into(), description: description.into(), local_build };
    SettingsView {
        sections: ["Provider", "Auth", "Tools", "Policy", "Memory", "Router", "Frontend", "Keys", "Callbacks", "Theme"].iter().map(|s| s.to_string()).collect(),
        section: 0,
        focus: SettingsPane::Sections,
        providers: vec![
            p("Anthropic API", "Claude, official API", false),
            p("OpenAI-compatible", "OpenAI, Azure, Groq, OpenRouter, ...", false),
            p("Local", "Ollama, vLLM, LM Studio, ...", false),
            // A user's own plugin (INV-9: a subscription login is never shipped; journal#248).
            p("claude-subscription", "your own build of a subscription login", true),
            p("custom-provider-acme", "Acme internal provider (experimental)", true),
        ],
        provider: 0,
        cursor: 0,
        model: MODELS[0].into(),
        timeout_s: 60,
        fallback: true,
        callbacks: CallbacksPage::default(),
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

/// Fills `r` with `bg` and draws a rounded frame in `line` around it. The frame's cells keep
/// the colour of what is under them, so the fill never sticks out past the line; the inside is
/// reset completely, so nothing under it (an underline, a bold) shows through.
fn modal(f: &mut Frame, r: Rect, line: Color, bg: Color) -> Rect {
    let buf = f.buffer_mut();
    for y in r.y..r.y + r.height {
        for x in r.x..r.x + r.width {
            if buf.area.contains((x, y).into()) {
                let keep = buf[(x, y)].bg;
                buf[(x, y)].reset();
                buf[(x, y)].set_bg(keep);
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
    }
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
    text(f, x, y, iw, vec![Span::styled("Always allow creates a persistent rule for this command pattern.", Style::default().fg(t.text_muted).add_modifier(Modifier::ITALIC))], bg);
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
    let h = 30u16.min(area.height);
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

fn provider_page(f: &mut Frame, r: Rect, s: &SettingsView, t: &Theme, bg: Color, out: &mut Drawn) {
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
        if under && !p.local_build {
            spans.push(Span::styled(format!("   {}", p.description), Style::default().fg(t.text_muted)));
        }
        let row = Rect::new(group.x, y, group.width, 1);
        text(f, row.x, row.y, row.width, spans, row_bg);
        if p.local_build {
            let pill = crate::transcript::pill_spans("local build", t.text, t.surface_raised, row_bg);
            let pw: u16 = 13;
            f.render_widget(Paragraph::new(Line::from(pill)), Rect::new(row.x + row.width - pw - 1, y, pw, 1));
        }
        out.hits.push(Hit { rect: row, target: Target::Provider(i) });
        y += 1;
    }
    // Fields.
    let mut y = group.y + group.height + 1;
    rule(f, r.x, y, r.width, t, bg);
    y += 2;
    let n = s.providers.len();
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
        field("Default model", bracket(s.model.clone(), true), 0),
        field("Timeout", bracket(format!("{}s", s.timeout_s), false), 1),
        field("Fallback on error", check, 2),
    ] {
        if y >= r.y + r.height {
            break;
        }
        text(f, r.x, y, r.width, spans, row_bg);
        y += 2;
    }
}

/// What a key does while an overlay is open. Returns None when no overlay is open.
pub fn key(w: &mut Workspace, k: Key) -> Option<Outcome> {
    let overlay = w.overlay.as_mut()?;
    match overlay {
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
            Key::Esc => w.overlay = None,
            Key::Ctrl('s') => {
                // Save: the session's status line shows the chosen adapter and model.
                let adapter = s.providers[s.provider].name.to_lowercase().replace(' ', "-");
                let model = s.model.clone();
                if w.live {
                    let provider = s.providers[s.provider].name.clone();
                    w.overlay = None;
                    w.outbox.push(Outgoing::SaveSettings { provider, model });
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
                SettingsPane::Page => s.cursor = (s.cursor + 1).min(s.providers.len() + 2),
            },
            Key::Char(' ') | Key::Enter if s.focus == SettingsPane::Page => activate(s),
            Key::Enter => s.focus = SettingsPane::Page,
            _ => {}
        },
    }
    Some(Outcome::Stay)
}

/// Space / Enter on the page row under the cursor: pick a provider, cycle a field, toggle a box.
fn activate(s: &mut SettingsView) {
    let n = s.providers.len();
    match s.cursor {
        i if i < n => s.provider = i,
        i if i == n => {
            let at = MODELS.iter().position(|m| *m == s.model).unwrap_or(0);
            s.model = MODELS[(at + 1) % MODELS.len()].into();
        }
        i if i == n + 1 => {
            let at = TIMEOUTS.iter().position(|v| *v == s.timeout_s).unwrap_or(0);
            s.timeout_s = TIMEOUTS[(at + 1) % TIMEOUTS.len()];
        }
        _ => s.fallback = !s.fallback,
    }
}

/// The user answered the permission prompt: the call shows in the transcript with the answer.
pub fn answer(w: &mut Workspace, choice: usize) {
    let Some(Overlay::Permission(p)) = w.overlay.take() else { return };
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
            s.provider = i;
            s.cursor = i;
            s.focus = SettingsPane::Page;
        }
        (Some(_), Target::CloseOverlay) => w.overlay = None,
        _ => {}
    }
    Outcome::Stay
}

#[cfg(test)]
mod tests {
    use super::*;
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
        key(&mut w, Key::Ctrl('s'));
        assert!(w.overlay.is_none(), "Ctrl+S saves and closes");
        assert!(w.view().status.fields.iter().any(|(k, v)| k == "ADAPTER" && v == "openai-compatible"));
    }
}
