use crate::cache::CacheStore;
use crate::cli::PercentStyle;
use crate::dashboard_prefs::{
    color_rgb, DashboardField, DashboardPreferences, DashboardProvider, ProviderPreference,
};
use crate::model::{format_percent, Provider, ProviderSnapshot, Severity};
use crate::opencode::LocalUsage;
use crate::presentation::{dashboard_segments_filtered, RowStyle};
use anyhow::{Context, Result};
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
    MouseButton, MouseEventKind,
};
use crossterm::style::{Color, ResetColor, SetBackgroundColor, SetForegroundColor};
use crossterm::terminal::{
    self, disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::{cursor, execute};
use std::fs;
use std::io::{self, IsTerminal, Write};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

const OUTER_BACKGROUND: Color = Color::Rgb {
    r: 14,
    g: 16,
    b: 20,
};
const PANEL_BACKGROUND: Color = Color::Rgb {
    r: 27,
    g: 30,
    b: 37,
};
const TEXT: Color = Color::Rgb {
    r: 232,
    g: 237,
    b: 247,
};
const MUTED: Color = Color::Rgb {
    r: 143,
    g: 155,
    b: 176,
};
const CYAN: Color = Color::Rgb {
    r: 83,
    g: 191,
    b: 230,
};
const GREEN: Color = Color::Rgb {
    r: 130,
    g: 217,
    b: 120,
};
const AMBER: Color = Color::Rgb {
    r: 228,
    g: 185,
    b: 87,
};
const RED: Color = Color::Rgb {
    r: 241,
    g: 111,
    b: 126,
};
const SETTINGS_FOOTER_PREFIX: &str = "  ";
const SETTINGS_LINK_LABEL: &str = "settings";

type StyledLine = Vec<(Color, String)>;
type DashboardRefreshUpdate = (bool, Option<Option<LocalUsage>>);

pub fn run() -> Result<()> {
    let cache = CacheStore::from_env()?;
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        let preferences = DashboardPreferences::load_or_default(&cache);
        let opencode_usage = crate::refresh::refresh_dashboard(&cache, &preferences, false)
            .ok()
            .flatten()
            .flatten();
        print_snapshot(&cache, &preferences, opencode_usage)?;
        return Ok(());
    }
    let (refresh_requests, refresh_updates) = start_dashboard_refresh_worker(cache.clone());
    let _ = refresh_requests.send(false);

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    let result = execute!(
        stdout,
        EnterAlternateScreen,
        EnableMouseCapture,
        cursor::Hide
    )
    .context("enter dashboard screen")
    .and_then(|_| interactive(&cache, &refresh_requests, &refresh_updates));
    let terminal_cleanup = execute!(
        stdout,
        ResetColor,
        cursor::Show,
        DisableMouseCapture,
        LeaveAlternateScreen
    )
    .context("leave dashboard screen");
    let raw_cleanup = disable_raw_mode().context("disable dashboard raw mode");
    result.and(terminal_cleanup).and(raw_cleanup)
}

/// Idle wait between frames. `poll` returns as soon as a key arrives, so this
/// bounds only how long an unattended popup sleeps, never how fast it reacts.
const IDLE_POLL: Duration = Duration::from_secs(1);

/// Repaint only when the rendered frame actually changed.
///
/// The popup stays open for as long as the user leaves it there. Clearing and
/// redrawing the whole screen several times a second flickers and re-reads
/// every cached snapshot for nothing: the numbers only move when a refresh
/// lands or a reset countdown ticks over a minute boundary.
fn interactive(
    cache: &CacheStore,
    refresh_requests: &Sender<bool>,
    refresh_updates: &Receiver<DashboardRefreshUpdate>,
) -> Result<()> {
    let mut painted: Option<String> = None;
    let mut scroll_offset = 0;
    let mut opencode_usage = None;
    let refresh_interval = Duration::from_secs(cache.watch_interval_seconds());
    let mut refreshed_at = Instant::now();
    let mut refresh_pending = true;
    loop {
        while let Ok((completed, refreshed_usage)) = refresh_updates.try_recv() {
            refresh_pending = false;
            if completed {
                refreshed_at = Instant::now();
                if let Some(usage) = refreshed_usage {
                    opencode_usage = usage;
                }
            }
        }
        if !refresh_pending && refreshed_at.elapsed() >= refresh_interval {
            refresh_pending = refresh_requests.send(false).is_ok();
        }
        let (width, height) = terminal::size().unwrap_or((78, 24));
        let (frame, max_scroll, footer_row) =
            render_terminal_scrolled(cache, width, height, opencode_usage, scroll_offset)?;
        scroll_offset = scroll_offset.min(max_scroll);
        if painted.as_deref() != Some(frame.as_str()) {
            print!("{}{frame}", repaint_prefix());
            io::stdout().flush()?;
            painted = Some(frame);
        }
        if event::poll(IDLE_POLL)? {
            match event::read()? {
                Event::Key(key) => {
                    if key.kind != KeyEventKind::Press {
                        continue;
                    }
                    if key.code == KeyCode::Char('c')
                        && key.modifiers.contains(KeyModifiers::CONTROL)
                    {
                        return Ok(());
                    }
                    let page = usize::from(height.max(2) / 2);
                    match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                        KeyCode::Char('s') => open_settings_from_dashboard(),
                        KeyCode::Up | KeyCode::Char('k') => {
                            scroll_offset = scroll_offset.saturating_sub(1)
                        }
                        KeyCode::Down | KeyCode::Char('j') => {
                            scroll_offset = scroll_offset.saturating_add(1).min(max_scroll)
                        }
                        KeyCode::PageUp => scroll_offset = scroll_offset.saturating_sub(page),
                        KeyCode::PageDown => {
                            scroll_offset = scroll_offset.saturating_add(page).min(max_scroll)
                        }
                        KeyCode::Home => scroll_offset = 0,
                        KeyCode::End => scroll_offset = max_scroll,
                        KeyCode::Char('r') if refresh_requests.send(true).is_ok() => {
                            refresh_pending = true
                        }
                        _ => {}
                    }
                }
                Event::Mouse(mouse) => match mouse.kind {
                    MouseEventKind::Down(MouseButton::Left)
                        if settings_link_hit(mouse.column, mouse.row, footer_row, width) =>
                    {
                        open_settings_from_dashboard();
                    }
                    MouseEventKind::ScrollUp => scroll_offset = scroll_offset.saturating_sub(1),
                    MouseEventKind::ScrollDown => {
                        scroll_offset = scroll_offset.saturating_add(1).min(max_scroll)
                    }
                    _ => {}
                },
                _ => {}
            }
        }
    }
}

fn open_settings_from_dashboard() {
    if let Err(error) = crate::herdr::invoke_settings_action() {
        let _ = crate::herdr::notify("QuotaDeck", &format!("Could not open settings: {error}"));
    }
}

fn start_dashboard_refresh_worker(
    cache: CacheStore,
) -> (Sender<bool>, Receiver<DashboardRefreshUpdate>) {
    let (request_tx, request_rx) = mpsc::channel();
    let (update_tx, update_rx) = mpsc::channel();
    thread::spawn(move || {
        while let Ok(mut force) = request_rx.recv() {
            while let Ok(queued_force) = request_rx.try_recv() {
                force |= queued_force;
            }
            let preferences = DashboardPreferences::load_or_default(&cache);
            let refreshed = loop {
                match crate::refresh::refresh_dashboard(&cache, &preferences, force) {
                    Ok(Some(usage)) => break Some(usage),
                    Ok(None) if force => thread::sleep(Duration::from_millis(250)),
                    Ok(None) | Err(_) => break None,
                }
            };
            let update = match refreshed {
                Some(usage) => (
                    true,
                    preferences
                        .get(DashboardProvider::OpenCode)
                        .show
                        .then_some(usage),
                ),
                None => (false, None),
            };
            if update_tx.send(update).is_err() {
                break;
            }
        }
    });
    (request_tx, update_rx)
}

fn print_snapshot(
    cache: &CacheStore,
    preferences: &DashboardPreferences,
    opencode_usage: Option<LocalUsage>,
) -> Result<()> {
    print!(
        "{}",
        render_snapshot_with_preferences(
            cache,
            CacheStore::now_unix(),
            opencode_usage,
            preferences,
        )?
    );
    Ok(())
}

#[cfg(test)]
fn render_snapshot_with_opencode(
    cache: &CacheStore,
    now: u64,
    opencode_usage: Option<LocalUsage>,
) -> Result<String> {
    render_snapshot_with_preferences(
        cache,
        now,
        opencode_usage,
        &DashboardPreferences::load_or_default(cache),
    )
}

fn render_snapshot_with_preferences(
    cache: &CacheStore,
    now: u64,
    opencode_usage: Option<LocalUsage>,
    preferences: &DashboardPreferences,
) -> Result<String> {
    let style = RowStyle::new(
        cache.percent_style().unwrap_or_default(),
        cache.brand_glyphs().unwrap_or_default(),
    );
    let mut output = String::from("QuotaDeck\r\n\r\n");
    for (provider, snapshot) in dashboard_rows(cache, preferences, now)? {
        let preference = preferences.get(provider);
        output.push_str(&match provider {
            DashboardProvider::OpenCode => render_opencode_usage(opencode_usage, style, preference),
            _ => render_quota_provider(
                provider.quota_provider().expect("quota provider"),
                snapshot.as_ref(),
                now,
                style,
                preference,
            ),
        });
        output.push_str("\r\n");
    }
    Ok(output)
}

pub fn render_provider(
    provider: Provider,
    snapshot: Option<&ProviderSnapshot>,
    now_unix: u64,
    style: impl Into<RowStyle>,
) -> String {
    let preference = ProviderPreference::defaults(DashboardProvider::from_quota_provider(provider));
    render_quota_provider(provider, snapshot, now_unix, style, &preference)
}

fn render_quota_provider(
    provider: Provider,
    snapshot: Option<&ProviderSnapshot>,
    now_unix: u64,
    style: impl Into<RowStyle>,
    preference: &ProviderPreference,
) -> String {
    let style = style.into();
    let values = provider_segments(provider, snapshot, now_unix, style.percent, preference)
        .into_iter()
        .map(|(value, _)| value)
        .collect::<Vec<_>>()
        .join(" · ");
    format!(
        "{}  {values}",
        style.glyphs.label(provider, provider.display_name())
    )
}

fn dashboard_rows(
    cache: &CacheStore,
    preferences: &DashboardPreferences,
    now: u64,
) -> Result<Vec<(DashboardProvider, Option<ProviderSnapshot>)>> {
    let mut rows = Vec::new();
    for preference in &preferences.providers {
        if !preference.show {
            continue;
        }
        let mut snapshot = match preference.provider {
            DashboardProvider::OpenCode => None,
            DashboardProvider::Omp => load_latest_omp(cache)?,
            provider => crate::refresh::load_usable_snapshot(
                cache,
                provider.quota_provider().expect("quota provider"),
            )?,
        };
        if let Some(provider) = preference.provider.quota_provider() {
            let failure = cache.refresh_problem(provider);
            if failure.is_some() && snapshot.is_none() {
                snapshot = Some(ProviderSnapshot::new(provider, Vec::new(), now));
            }
            if let Some(snapshot) = snapshot.as_mut() {
                snapshot.refresh_warning = cache.refresh_warning(provider, Some(snapshot), now);
            }
        }
        rows.push((preference.provider, snapshot));
    }
    if cache.agent_order().is_some_and(|order| order.is_quota()) {
        rows.sort_by(
            |(left_provider, left_snapshot), (right_provider, right_snapshot)| {
                let headroom = |provider, snapshot: Option<&ProviderSnapshot>| {
                    tightest_visible_window(provider, snapshot, preferences.get(provider))
                        .map(|window| window.remaining_percent)
                };
                match (
                    headroom(*left_provider, left_snapshot.as_ref()),
                    headroom(*right_provider, right_snapshot.as_ref()),
                ) {
                    (Some(left), Some(right)) => left.total_cmp(&right),
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (None, None) => std::cmp::Ordering::Equal,
                }
            },
        );
    }
    Ok(rows)
}

/// OMP targets are keyed by an irreversible provider-id hash. The dashboard
/// needs one useful OMP row, so select the newest sanitized cached snapshot
/// without opening OMP's credential database or attempting to reverse the id.
fn load_latest_omp(cache: &CacheStore) -> Result<Option<ProviderSnapshot>> {
    let entries = match fs::read_dir(cache.root()) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("read cache directory {}", cache.root().display()))
        }
    };
    let mut latest: Option<ProviderSnapshot> = None;
    for entry in entries {
        let entry = entry.context("read cached OMP entry")?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("omp-usage-") || !name.ends_with(".omp-store.json") {
            continue;
        }
        let bytes =
            fs::read(entry.path()).with_context(|| format!("read cached OMP snapshot {name}"))?;
        let snapshot: ProviderSnapshot = serde_json::from_slice(&bytes)
            .with_context(|| format!("parse cached OMP snapshot {name}"))?;
        if snapshot.provider != Provider::Omp {
            continue;
        }
        if latest
            .as_ref()
            .is_none_or(|current| snapshot.fetched_at_unix > current.fetched_at_unix)
        {
            latest = Some(snapshot);
        }
    }
    Ok(latest)
}

fn render_opencode_usage(
    usage: Option<LocalUsage>,
    style: RowStyle,
    preference: &ProviderPreference,
) -> String {
    let values = opencode_segments(usage, preference)
        .into_iter()
        .map(|(value, _)| value)
        .collect::<Vec<_>>()
        .join(" · ");
    format!(
        "{}  {values}",
        style.glyphs.label(Provider::OpenCodeGo, "OpenCode")
    )
}

#[cfg(test)]
fn render_terminal(
    cache: &CacheStore,
    width: u16,
    height: u16,
    opencode_usage: Option<LocalUsage>,
) -> Result<String> {
    render_terminal_scrolled(cache, width, height, opencode_usage, 0).map(|(frame, _, _)| frame)
}

fn render_terminal_scrolled(
    cache: &CacheStore,
    width: u16,
    height: u16,
    opencode_usage: Option<LocalUsage>,
    scroll_offset: usize,
) -> Result<(String, usize, u16)> {
    crossterm::style::force_color_output(true);
    let now = CacheStore::now_unix();
    let style = RowStyle::new(
        cache.percent_style().unwrap_or_default(),
        cache.brand_glyphs().unwrap_or_default(),
    );
    let preferences = DashboardPreferences::load_or_default(cache);
    let rows = dashboard_rows(cache, &preferences, now)?;
    let width = usize::from(width.max(1));
    let panel_width = width.saturating_sub(4).max(1);
    let mut lines: Vec<(bool, StyledLine)> = vec![
        (false, Vec::new()),
        (true, Vec::new()),
        (true, vec![(CYAN, "  QuotaDeck".to_string())]),
        (true, Vec::new()),
    ];
    for (provider, snapshot) in &rows {
        let preference = preferences.get(*provider);
        let row_lines = match provider {
            DashboardProvider::OpenCode => {
                opencode_lines(opencode_usage, style, preference, panel_width)
            }
            _ => provider_lines(
                provider.quota_provider().expect("quota provider"),
                snapshot.as_ref(),
                now,
                style,
                preference,
                panel_width,
            ),
        };
        lines.extend(row_lines.into_iter().map(|line| (true, line)));
    }
    lines.push((true, Vec::new()));

    let height = usize::from(height.max(1));
    let body_height = height.saturating_sub(1);
    let max_scroll = lines.len().saturating_sub(body_height);
    let scroll_offset = scroll_offset.min(max_scroll);
    let footer_tail = if max_scroll > 0 {
        format!(
            " [s/click] · ↑/↓ {}/{} · Pg · q",
            scroll_offset + 1,
            max_scroll + 1
        )
    } else {
        tightest(&rows, &preferences)
            .map(|value| format!(" [s/click] · q close · {value}"))
            .unwrap_or_else(|| " [s/click] · q close".to_string())
    };
    let footer_row = if max_scroll > 0 {
        height.saturating_sub(1)
    } else {
        lines.len()
    } as u16;
    let footer = (
        true,
        vec![
            (MUTED, SETTINGS_FOOTER_PREFIX.to_string()),
            (CYAN, SETTINGS_LINK_LABEL.to_string()),
            (MUTED, footer_tail),
        ],
    );
    if max_scroll > 0 {
        lines = lines
            .into_iter()
            .skip(scroll_offset)
            .take(body_height)
            .collect();
        lines.push(footer);
    } else {
        lines.push(footer);
        lines.resize_with(height, || (false, Vec::new()));
    }

    let mut output = String::new();
    push_ansi(&mut output, SetBackgroundColor(OUTER_BACKGROUND));
    let line_count = lines.len();
    for (index, (inside_panel, line)) in lines.into_iter().enumerate() {
        push_styled_line(&mut output, &line, width, inside_panel);
        if index + 1 < line_count {
            output.push_str("\r\n");
        }
    }
    push_ansi(&mut output, ResetColor);
    Ok((output, max_scroll, footer_row))
}

fn settings_link_hit(column: u16, row: u16, footer_row: u16, width: u16) -> bool {
    let margin = u16::from(width >= 5) * 2;
    let start = margin + SETTINGS_FOOTER_PREFIX.chars().count() as u16;
    row == footer_row
        && (start..start + SETTINGS_LINK_LABEL.chars().count() as u16).contains(&column)
}

fn provider_lines(
    provider: Provider,
    snapshot: Option<&ProviderSnapshot>,
    now_unix: u64,
    style: RowStyle,
    preference: &ProviderPreference,
    width: usize,
) -> Vec<StyledLine> {
    let label = format!(
        "  {}",
        style.glyphs.label(provider, provider.display_name())
    );
    let segments = provider_segments(provider, snapshot, now_unix, style.percent, preference);
    summary_lines(label, provider_color(preference), segments, width)
}

fn opencode_lines(
    usage: Option<LocalUsage>,
    style: RowStyle,
    preference: &ProviderPreference,
    width: usize,
) -> Vec<StyledLine> {
    summary_lines(
        format!("  {}", style.glyphs.label(Provider::OpenCodeGo, "OpenCode")),
        provider_color(preference),
        opencode_segments(usage, preference),
        width,
    )
}

fn summary_lines(
    label: String,
    label_color: Color,
    segments: Vec<(String, Severity)>,
    width: usize,
) -> Vec<StyledLine> {
    const RIGHT_PADDING: usize = 2;
    let values_width = segments
        .iter()
        .map(|(value, _)| value.chars().count())
        .sum::<usize>()
        + 3 * segments.len().saturating_sub(1);
    let label_width = label.chars().count();
    let gap = width.saturating_sub(label_width + values_width + RIGHT_PADDING);
    if gap > 0 {
        let mut line = vec![(label_color, label)];
        line.push((TEXT, " ".repeat(gap)));
        append_segments(&mut line, segments);
        return vec![line];
    }

    let mut lines = vec![vec![(label_color, label)]];
    for (value, severity) in segments {
        let value_width = value.chars().count();
        let padding = width.saturating_sub(value_width + RIGHT_PADDING);
        lines.push(vec![
            (TEXT, " ".repeat(padding)),
            (severity_color(severity), value),
        ]);
    }
    lines
}

fn opencode_segments(
    usage: Option<LocalUsage>,
    preference: &ProviderPreference,
) -> Vec<(String, Severity)> {
    match usage {
        Some(usage) => [
            preference.has(DashboardField::Tokens).then(|| {
                (
                    format!("30d {} tok", format_token_count(usage.tokens)),
                    Severity::Normal,
                )
            }),
            preference
                .has(DashboardField::Spend)
                .then(|| (format!("spent ${:.2}", usage.cost_usd), Severity::Normal)),
        ]
        .into_iter()
        .flatten()
        .collect(),
        None if preference.fields.is_empty() => Vec::new(),
        None => vec![("30d N/A".to_string(), Severity::Unknown)],
    }
}

fn format_token_count(tokens: u64) -> String {
    if tokens >= 1_000_000_000 {
        format!("{:.1}B", tokens as f64 / 1_000_000_000.0)
    } else if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.1}K", tokens as f64 / 1_000.0)
    } else {
        tokens.to_string()
    }
}

fn provider_segments(
    provider: Provider,
    snapshot: Option<&ProviderSnapshot>,
    now_unix: u64,
    style: PercentStyle,
    preference: &ProviderPreference,
) -> Vec<(String, Severity)> {
    if let Some(snapshot) = snapshot {
        if let Some(warning) = &snapshot.refresh_warning {
            return vec![(warning.clone(), Severity::Warning)];
        }
        let segments = dashboard_segments_filtered(snapshot, now_unix, style, preference);
        if !segments.is_empty() {
            return segments;
        }
    }
    match provider {
        Provider::Claude | Provider::Agy => [
            (preference.has(DashboardField::ShortPercent)
                || preference.has(DashboardField::ShortReset))
            .then(|| ("5h N/A".to_string(), Severity::Unknown)),
            (preference.has(DashboardField::LongPercent)
                || preference.has(DashboardField::LongReset))
            .then(|| ("7d N/A".to_string(), Severity::Unknown)),
        ]
        .into_iter()
        .flatten()
        .collect(),
        Provider::OpenRouter if !preference.fields.is_empty() => {
            vec![("credentials unavailable".to_string(), Severity::Unknown)]
        }
        _ if preference.fields.is_empty() => Vec::new(),
        _ => vec![("N/A".to_string(), Severity::Unknown)],
    }
}

fn append_segments(line: &mut StyledLine, segments: Vec<(String, Severity)>) {
    for (index, (value, severity)) in segments.into_iter().enumerate() {
        if index > 0 {
            line.push((MUTED, " · ".to_string()));
        }
        line.push((severity_color(severity), value));
    }
}

fn push_styled_line(output: &mut String, line: &StyledLine, width: usize, inside_panel: bool) {
    let margin = if inside_panel && width >= 5 { 2 } else { 0 };
    let content_width = width.saturating_sub(margin * 2);
    let mut remaining = content_width;
    push_ansi(output, SetBackgroundColor(OUTER_BACKGROUND));
    output.push_str(&" ".repeat(margin));
    if inside_panel {
        push_ansi(output, SetBackgroundColor(PANEL_BACKGROUND));
    }
    for (color, text) in line {
        if remaining == 0 {
            break;
        }
        let text = text.chars().take(remaining).collect::<String>();
        remaining = remaining.saturating_sub(text.chars().count());
        push_ansi(output, SetForegroundColor(*color));
        output.push_str(&text);
    }
    output.push_str(&" ".repeat(remaining));
    if inside_panel {
        push_ansi(output, SetBackgroundColor(OUTER_BACKGROUND));
    }
    output.push_str(&" ".repeat(margin));
}

fn repaint_prefix() -> String {
    let mut output = String::new();
    push_ansi(&mut output, SetBackgroundColor(OUTER_BACKGROUND));
    push_ansi(&mut output, terminal::Clear(terminal::ClearType::All));
    push_ansi(&mut output, cursor::MoveTo(0, 0));
    output
}

fn push_ansi(output: &mut String, command: impl crossterm::Command) {
    command
        .write_ansi(output)
        .expect("writing ANSI to a String cannot fail");
}

fn tightest(
    rows: &[(DashboardProvider, Option<ProviderSnapshot>)],
    preferences: &DashboardPreferences,
) -> Option<String> {
    rows.iter()
        .filter_map(|(provider, snapshot)| {
            tightest_visible_window(*provider, snapshot.as_ref(), preferences.get(*provider))
                .map(|window| (provider, window))
        })
        .min_by(|(_, left), (_, right)| left.remaining_percent.total_cmp(&right.remaining_percent))
        .map(|(provider, window)| {
            format!(
                "{} {} {}%",
                provider.label(),
                window.display_label(),
                format_percent(window.remaining_percent)
            )
        })
}

fn tightest_visible_window<'a>(
    provider: DashboardProvider,
    snapshot: Option<&'a ProviderSnapshot>,
    preference: &ProviderPreference,
) -> Option<&'a crate::model::UsageWindow> {
    snapshot
        .filter(|snapshot| snapshot.refresh_warning.is_none())?
        .windows
        .iter()
        .filter(|window| percentage_visible(provider, window.kind, preference))
        .min_by(|left, right| left.remaining_percent.total_cmp(&right.remaining_percent))
}

fn percentage_visible(
    provider: DashboardProvider,
    kind: crate::model::WindowKind,
    preference: &ProviderPreference,
) -> bool {
    use crate::model::WindowKind;
    use DashboardField::*;
    preference.has(match (provider, kind) {
        (DashboardProvider::Hermes, WindowKind::Monthly) => PlanPercent,
        (DashboardProvider::Hermes, _) => TopUpPercent,
        (DashboardProvider::OpenRouter, _) => CreditsPercent,
        (_, WindowKind::FiveHour) => ShortPercent,
        _ => LongPercent,
    })
}

fn severity_color(severity: Severity) -> Color {
    match severity {
        Severity::Normal => GREEN,
        Severity::Warning => AMBER,
        Severity::Danger => RED,
        Severity::Unknown => MUTED,
    }
}

fn provider_color(preference: &ProviderPreference) -> Color {
    let (r, g, b) = color_rgb(&preference.color);
    Color::Rgb { r, g, b }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{BillingTarget, ResetAt, UsageWindow, WindowKind};
    use tempfile::tempdir;

    fn window(kind: WindowKind, used: f64, reset: Option<u64>) -> UsageWindow {
        UsageWindow::new(kind, used, reset.map(ResetAt::from_unix_seconds)).unwrap()
    }

    #[test]
    fn failed_expired_and_recovered_refreshes_never_show_old_quota_as_current() {
        let directory = tempdir().unwrap();
        let cache = CacheStore::new(directory.path());
        let provider = Provider::OpenRouter;
        let mut snapshot = ProviderSnapshot::new(
            provider,
            vec![window(WindowKind::Monthly, 61.0, None)],
            1000,
        );
        cache.save(&snapshot).unwrap();
        cache.set_refresh_problem(provider, Some("login")).unwrap();
        let frame = render_snapshot_with_opencode(&cache, 1001, None).unwrap();
        assert!(frame.contains("sign in again"));
        assert!(!frame.contains("39%"));
        cache.set_refresh_problem(provider, Some("failed")).unwrap();
        assert!(render_snapshot_with_opencode(&cache, 1002, None)
            .unwrap()
            .contains("refresh failed"));
        cache.set_refresh_problem(provider, None).unwrap();
        assert!(render_snapshot_with_opencode(&cache, 1201, None)
            .unwrap()
            .contains("stale; last update"));
        snapshot.fetched_at_unix = 1202;
        cache.save(&snapshot).unwrap();
        let frame = render_snapshot_with_opencode(&cache, 1203, None).unwrap();
        assert!(frame.contains("39%"));
        assert!(!frame.contains("sign in again"));
        assert!(!frame.contains("refresh failed"));
        assert!(cache
            .set_refresh_problem(provider, Some("secret-token"))
            .is_err());
        assert!(!serde_json::to_string(&snapshot)
            .unwrap()
            .contains("refresh_warning"));
    }

    #[test]
    fn renders_compact_values_without_status_prose() {
        let snapshot = ProviderSnapshot::new(
            Provider::Claude,
            vec![
                window(WindowKind::FiveHour, 58.0, Some(14_820)),
                window(WindowKind::Weekly, 27.0, Some(183_600)),
            ],
            1,
        );
        let rendered = render_provider(
            Provider::Claude,
            Some(&snapshot),
            0,
            PercentStyle::default(),
        );
        assert_eq!(
            rendered,
            "\u{e1a0} Claude  5h 42% reset 4h07m · 7d 73% reset 2d3h"
        );
        assert!(!rendered.contains("WARN"));
        assert!(!rendered.contains("LOW"));
        assert!(!rendered.contains("left"));
    }

    #[test]
    fn cached_scoped_collectors_appear_once_and_latest_omp_wins() {
        let directory = tempdir().unwrap();
        let cache = CacheStore::new(directory.path());
        let mut preferences = DashboardPreferences::default();
        preferences.get_mut(DashboardProvider::OpenCodeGo).show = true;
        preferences.save(&cache).unwrap();
        cache
            .save(&ProviderSnapshot::new(
                Provider::OpenCodeGo,
                vec![window(WindowKind::Monthly, 30.0, None)],
                1,
            ))
            .unwrap();
        cache
            .save_target(
                &BillingTarget::omp("older"),
                &ProviderSnapshot::new(
                    Provider::Omp,
                    vec![window(WindowKind::Weekly, 80.0, None)],
                    1,
                ),
            )
            .unwrap();
        cache
            .save_target(
                &BillingTarget::omp("newer"),
                &ProviderSnapshot::new(
                    Provider::Omp,
                    vec![window(WindowKind::Weekly, 31.0, None)],
                    2,
                ),
            )
            .unwrap();

        let rendered = render_snapshot_with_opencode(&cache, 0, None).unwrap();
        assert_eq!(rendered.matches("OpenCode Go").count(), 1, "{rendered}");
        assert_eq!(rendered.matches("OMP").count(), 1, "{rendered}");
        assert!(rendered.contains("OMP  7d 69%"), "{rendered}");
        assert!(!rendered.contains("OMP  7d 20%"), "{rendered}");
    }

    #[test]
    fn dashboard_never_borrows_an_unscoped_session_window() {
        let directory = tempdir().unwrap();
        let cache = CacheStore::new(directory.path());
        let mut snapshot = ProviderSnapshot::new(Provider::Claude, vec![], CacheStore::now_unix());
        snapshot.session_windows.insert(
            "live".to_string(),
            vec![window(WindowKind::Weekly, 51.0, Some(183_600))],
        );
        cache.save(&snapshot).unwrap();

        let rows = dashboard_rows(&cache, &DashboardPreferences::default(), 0).unwrap();
        let claude = rows
            .into_iter()
            .find(|(provider, _)| *provider == DashboardProvider::Claude)
            .and_then(|(_, snapshot)| snapshot)
            .unwrap();
        assert!(claude.windows.is_empty());
        let rendered = render_snapshot_with_opencode(&cache, 0, None).unwrap();
        assert!(!rendered.contains("49%"), "{rendered}");
    }

    #[test]
    fn quota_order_sorts_dashboard_rows_by_lowest_visible_headroom() {
        let directory = tempdir().unwrap();
        let cache = CacheStore::new(directory.path());
        cache
            .set_agent_order(crate::cli::AgentOrder::Quota)
            .unwrap();
        let now = CacheStore::now_unix();
        cache
            .save(&ProviderSnapshot::new(
                Provider::Codex,
                vec![window(WindowKind::FiveHour, 80.0, None)],
                now,
            ))
            .unwrap();
        cache
            .save(&ProviderSnapshot::new(
                Provider::Grok,
                vec![window(WindowKind::Weekly, 10.0, None)],
                now,
            ))
            .unwrap();

        let rows = dashboard_rows(&cache, &DashboardPreferences::default(), 0).unwrap();
        assert_eq!(rows[0].0, DashboardProvider::Codex);
        assert_eq!(rows[1].0, DashboardProvider::Grok);
    }

    #[test]
    fn interactive_rows_use_independent_window_colors_and_an_opaque_panel() {
        let directory = tempdir().unwrap();
        let cache = CacheStore::new(directory.path());
        cache
            .save(&ProviderSnapshot::new(
                Provider::Claude,
                vec![
                    window(WindowKind::FiveHour, 38.0, None),
                    window(WindowKind::Weekly, 76.0, None),
                ],
                CacheStore::now_unix(),
            ))
            .unwrap();
        let frame = render_terminal(&cache, 78, 18, None).unwrap();
        assert!(frame.starts_with("\u{1b}[48;2;14;16;20m"), "{frame:?}");
        assert!(frame.contains("\u{1b}[48;2;27;30;37m"), "{frame:?}");
        assert!(frame.contains("settings"), "{frame:?}");
        assert!(frame.contains("[s/click] · q close"), "{frame:?}");
        assert!(
            frame.contains("\u{1b}[38;2;130;217;120m5h 62%"),
            "{frame:?}"
        );
        assert!(frame.contains("\u{1b}[38;2;228;185;87m7d 24%"), "{frame:?}");
        assert_eq!(
            repaint_prefix(),
            "\u{1b}[48;2;14;16;20m\u{1b}[2J\u{1b}[1;1H"
        );
    }

    #[test]
    fn saved_display_choices_control_order_color_fields_and_footer() {
        let directory = tempdir().unwrap();
        let cache = CacheStore::new(directory.path());
        let mut preferences = DashboardPreferences::default();
        preferences.move_by(0, 2);
        preferences.get_mut(DashboardProvider::Codex).show = false;
        preferences.get_mut(DashboardProvider::Claude).color = "#010203".to_string();
        for preference in &mut preferences.providers {
            preference.fields.retain(|field| {
                !matches!(
                    field,
                    DashboardField::ShortPercent
                        | DashboardField::LongPercent
                        | DashboardField::PlanPercent
                        | DashboardField::TopUpPercent
                        | DashboardField::CreditsPercent
                )
            });
        }
        preferences.save(&cache).unwrap();
        cache
            .save(&ProviderSnapshot::new(
                Provider::Claude,
                vec![window(WindowKind::FiveHour, 95.0, Some(3_600))],
                CacheStore::now_unix(),
            ))
            .unwrap();

        let plain = render_snapshot_with_opencode(&cache, 0, None).unwrap();
        assert!(!plain.contains("Codex"), "{plain}");
        assert!(plain.find("Grok").unwrap() < plain.find("Claude").unwrap());
        let frame = render_terminal(&cache, 78, 18, None).unwrap();
        assert!(
            frame.contains("\u{1b}[38;2;1;2;3m  \u{e1a0} Claude"),
            "{frame:?}"
        );
        assert!(
            frame.contains("\u{1b}[38;2;241;111;126m5h reset"),
            "{frame:?}"
        );
        assert!(!frame.contains("tightest:"), "{frame:?}");
    }

    #[test]
    fn opencode_go_keeps_short_and_long_windows_and_money_keeps_severity() {
        let directory = tempdir().unwrap();
        let cache = CacheStore::new(directory.path());
        let mut preferences = DashboardPreferences::default();
        preferences.get_mut(DashboardProvider::OpenCodeGo).show = true;
        preferences.save(&cache).unwrap();
        cache
            .save(&ProviderSnapshot::new(
                Provider::OpenCodeGo,
                vec![
                    window(WindowKind::FiveHour, 10.0, None),
                    window(WindowKind::Weekly, 20.0, None),
                    window(WindowKind::Monthly, 30.0, None),
                ],
                1,
            ))
            .unwrap();
        let plain = render_snapshot_with_opencode(&cache, 0, None).unwrap();
        for expected in ["5h 90%", "7d 80%", "30d 70%"] {
            assert!(plain.contains(expected), "{plain}");
        }

        let top_up =
            window(WindowKind::Weekly, 95.0, None).with_source_window("top-up $1.00", None);
        let snapshot = ProviderSnapshot::new(Provider::Hermes, vec![top_up], 1);
        let preference = ProviderPreference::defaults(DashboardProvider::Hermes);
        let segments = provider_segments(
            Provider::Hermes,
            Some(&snapshot),
            0,
            PercentStyle::Remaining,
            &preference,
        );
        assert!(segments[0].0.contains('$'));
        assert_eq!(segments[0].1, Severity::Danger);
    }

    #[test]
    fn dashboard_honors_each_stored_brand_glyph_set() {
        let directory = tempdir().unwrap();
        let cache = CacheStore::new(directory.path());
        for (glyphs, claude, opencode, openrouter) in [
            (
                crate::brand::GlyphSet::IconFont,
                "\u{e1a0} Claude",
                "\u{e1a2} OpenCode",
                "\u{e1b2} OpenRouter",
            ),
            (
                crate::brand::GlyphSet::Unicode,
                "✳ Claude",
                "❑ OpenCode",
                "⇄ OpenRouter",
            ),
            (
                crate::brand::GlyphSet::Off,
                "Claude",
                "OpenCode",
                "OpenRouter",
            ),
        ] {
            cache.set_brand_glyphs(glyphs).unwrap();

            let snapshot = render_snapshot_with_opencode(&cache, 0, None).unwrap();
            assert!(
                snapshot.contains(&format!("\r\n{claude}  ")),
                "{snapshot:?}"
            );
            assert!(
                snapshot.contains(&format!("\r\n{opencode}  30d N/A")),
                "{snapshot:?}"
            );
            assert!(
                snapshot.contains(&format!("\r\n{openrouter}  ")),
                "{snapshot:?}"
            );

            let frame = render_terminal(&cache, 78, 18, None).unwrap();
            assert!(frame.contains(&format!("  {claude}")), "{frame:?}");
            assert!(frame.contains(&format!("  {opencode}")), "{frame:?}");
            assert!(frame.contains(&format!("  {openrouter}")), "{frame:?}");
        }
    }

    #[test]
    fn local_opencode_usage_is_labeled_as_spend_not_remaining_quota() {
        let directory = tempdir().unwrap();
        let cache = CacheStore::new(directory.path());
        let usage = Some(LocalUsage {
            tokens: 12_345_678,
            cost_usd: 4.567,
        });
        let snapshot = render_snapshot_with_opencode(&cache, 0, usage).unwrap();
        assert!(
            snapshot.contains("\u{e1a2} OpenCode  30d 12.3M tok · spent $4.57"),
            "{snapshot:?}"
        );
        assert!(!snapshot.contains("OpenCode  30d 12.3M tok · credits"));
    }

    #[test]
    fn plain_snapshot_is_ansi_free_and_returns_to_column_zero() {
        let directory = tempdir().unwrap();
        let rendered =
            render_snapshot_with_opencode(&CacheStore::new(directory.path()), 0, None).unwrap();
        assert!(rendered.contains("QuotaDeck\r\n\r\n\u{e1a0} Claude"));
        assert!(rendered.contains("\u{e1a2} OpenCode  30d N/A"));
        assert!(rendered.contains("OpenRouter  credentials unavailable"));
        assert!(!rendered.contains("Hermes •"), "{rendered:?}");
        assert!(!rendered.contains("OpenRouter •"), "{rendered:?}");
        assert!(rendered.contains("OMP  N/A"));
        assert!(!rendered.contains('\u{1b}'), "{rendered:?}");
        assert!(!rendered.contains("QuotaDeck\n"));
    }

    #[test]
    fn a_short_terminal_renders_exactly_its_height_without_scrolling() {
        let directory = tempdir().unwrap();
        let frame = render_terminal(&CacheStore::new(directory.path()), 78, 5, None).unwrap();
        assert_eq!(frame.matches("\r\n").count(), 4, "{frame:?}");
    }

    #[test]
    fn a_short_terminal_scrolls_the_current_dashboard_without_growing_history() {
        let directory = tempdir().unwrap();
        let cache = CacheStore::new(directory.path());
        let (top, max_scroll, _) = render_terminal_scrolled(&cache, 78, 5, None, 0).unwrap();
        let (bottom, same_max, _) =
            render_terminal_scrolled(&cache, 78, 5, None, usize::MAX).unwrap();

        assert!(max_scroll > 0);
        assert_eq!(same_max, max_scroll);
        assert_ne!(top, bottom);
        assert!(top.contains("QuotaDeck"), "{top:?}");
        assert!(bottom.contains("OMP"), "{bottom:?}");
        assert!(bottom.contains("↑/↓"), "{bottom:?}");
        assert_eq!(bottom.matches("\r\n").count(), 4, "{bottom:?}");
    }

    #[test]
    fn styled_lines_clip_before_the_terminal_wraps() {
        let mut output = String::new();
        push_styled_line(
            &mut output,
            &vec![(TEXT, "0123456789".to_string())],
            5,
            false,
        );
        assert!(output.contains("01234"), "{output:?}");
        assert!(!output.contains("012345"), "{output:?}");
    }

    #[test]
    fn only_the_settings_label_on_the_footer_is_clickable() {
        assert!(settings_link_hit(4, 20, 20, 78));
        assert!(settings_link_hit(11, 20, 20, 78));
        assert!(!settings_link_hit(3, 20, 20, 78));
        assert!(!settings_link_hit(12, 20, 20, 78));
        assert!(!settings_link_hit(4, 19, 20, 78));
    }
}
