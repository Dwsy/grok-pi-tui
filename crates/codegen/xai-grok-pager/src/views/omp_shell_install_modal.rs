//! Centered TUI progress modal shown when the user enables `OMP Shell backend`
//! in F2 settings and `grok-pi-shell-host` is not yet installed locally.
//!
//! Displays live download progress from the `omp-shell-host-latest` standalone
//! release, extraction & `--handshake` verification phases, and retry/cancel
//! controls. `pi_omp_shell` stays `false` until installation succeeds.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;
use unicode_width::UnicodeWidthStr;
use xai_grok_shell::util::omp_shell_install::{
    OmpShellInstallPhase, SharedOmpShellInstallProgress, omp_shell_install_path,
};

use crate::theme::Theme;
use crate::views::modal_window::word_wrap;
use crate::views::progress_bar::render_progress_bar;

const SPINNER_FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// State owned by `AppView` while an on-demand `grok-pi-shell-host` download is active
/// or displaying an error awaiting retry/dismissal.
#[derive(Debug)]
pub struct OmpShellInstallModalState {
    pub generation: u64,
    pub cancel: Arc<AtomicBool>,
    pub progress: SharedOmpShellInstallProgress,
    pub started_at: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OmpShellInstallOutcome {
    Retry,
    Cancel,
    Unchanged,
}

impl OmpShellInstallModalState {
    pub fn new(generation: u64) -> Self {
        Self {
            generation,
            cancel: Arc::new(AtomicBool::new(false)),
            progress: Arc::new(Mutex::new(OmpShellInstallPhase::Resolving)),
            started_at: Instant::now(),
        }
    }

    pub fn snapshot(&self) -> OmpShellInstallPhase {
        self.progress
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or(OmpShellInstallPhase::Resolving)
    }

    pub fn is_in_progress(&self) -> bool {
        matches!(
            self.snapshot(),
            OmpShellInstallPhase::Resolving
                | OmpShellInstallPhase::Downloading { .. }
                | OmpShellInstallPhase::Extracting
                | OmpShellInstallPhase::Verifying
        )
    }

    pub fn set_failed(&self, err: String) {
        if let Ok(mut guard) = self.progress.lock() {
            *guard = OmpShellInstallPhase::Failed(err);
        }
    }

    pub fn handle_key(&self, key: &KeyEvent) -> OmpShellInstallOutcome {
        match key.code {
            KeyCode::Esc => {
                self.cancel.store(true, Ordering::Relaxed);
                OmpShellInstallOutcome::Cancel
            }
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.cancel.store(true, Ordering::Relaxed);
                OmpShellInstallOutcome::Cancel
            }
            KeyCode::Char('q') if !self.is_in_progress() => {
                self.cancel.store(true, Ordering::Relaxed);
                OmpShellInstallOutcome::Cancel
            }
            KeyCode::Enter | KeyCode::Char('r')
                if matches!(self.snapshot(), OmpShellInstallPhase::Failed(_)) =>
            {
                OmpShellInstallOutcome::Retry
            }
            _ => OmpShellInstallOutcome::Unchanged,
        }
    }
}

fn format_bytes(bytes: u64) -> String {
    const MIB: f64 = 1024.0 * 1024.0;
    const KIB: f64 = 1024.0;
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / MIB)
    } else if bytes >= 1024 {
        format!("{:.0} KB", bytes as f64 / KIB)
    } else {
        format!("{bytes} B")
    }
}

fn truncate_to_width(s: &str, max_width: usize) -> String {
    if UnicodeWidthStr::width(s) <= max_width {
        return s.to_string();
    }
    if max_width <= 1 {
        return "…".to_string();
    }
    let mut out = String::new();
    let mut used = 0usize;
    for ch in s.chars() {
        let mut buf = [0u8; 4];
        let w = UnicodeWidthStr::width(ch.encode_utf8(&mut buf) as &str);
        if used + w + 1 > max_width {
            break;
        }
        out.push(ch);
        used += w;
    }
    out.push('…');
    out
}

/// Render the centered OMP Shell download/install progress dialog on top of the active view.
pub fn render_omp_shell_install_modal(
    buf: &mut Buffer,
    area: Rect,
    state: &OmpShellInstallModalState,
    theme: &Theme,
) {
    let phase = state.snapshot();
    let is_failed = matches!(phase, OmpShellInstallPhase::Failed(_));
    let dialog_height: u16 = if is_failed { 10 } else { 9 };
    let dialog_width: u16 = area.width.saturating_sub(4).clamp(36, 66);

    if area.height < dialog_height || area.width < 32 {
        if area.height >= 1 && area.width >= 16 {
            let hint = Line::from(Span::styled(
                "[Esc] cancel",
                Style::default().fg(theme.gray_dim),
            ));
            hint.render(Rect::new(area.x, area.y, area.width.min(16), 1), buf);
        }
        return;
    }

    let [_, dialog_h, _] = Layout::horizontal([
        Constraint::Min(0),
        Constraint::Length(dialog_width),
        Constraint::Min(0),
    ])
    .flex(Flex::Center)
    .areas(area);

    let [_, dialog, _] = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(dialog_height),
        Constraint::Min(0),
    ])
    .flex(Flex::Center)
    .areas(dialog_h);

    let bg = theme.bg_dark;
    let bg_style = Style::default().bg(bg);
    for y in dialog.y..dialog.y + dialog.height {
        for x in dialog.x..dialog.x + dialog.width {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_char(' ');
                cell.set_style(bg_style);
            }
        }
    }

    let border_fg = if is_failed {
        theme.accent_error
    } else {
        theme.accent_tool
    };
    let border_style = Style::default().fg(border_fg).bg(bg);
    let right = dialog.x + dialog.width - 1;
    let bottom = dialog.y + dialog.height - 1;

    if let Some(c) = buf.cell_mut((dialog.x, dialog.y)) {
        c.set_char('╭');
        c.set_style(border_style);
    }
    if let Some(c) = buf.cell_mut((right, dialog.y)) {
        c.set_char('╮');
        c.set_style(border_style);
    }
    if let Some(c) = buf.cell_mut((dialog.x, bottom)) {
        c.set_char('╰');
        c.set_style(border_style);
    }
    if let Some(c) = buf.cell_mut((right, bottom)) {
        c.set_char('╯');
        c.set_style(border_style);
    }
    for x in (dialog.x + 1)..right {
        if let Some(c) = buf.cell_mut((x, dialog.y)) {
            c.set_char('─');
            c.set_style(border_style);
        }
        if let Some(c) = buf.cell_mut((x, bottom)) {
            c.set_char('─');
            c.set_style(border_style);
        }
    }
    for y in (dialog.y + 1)..bottom {
        if let Some(c) = buf.cell_mut((dialog.x, y)) {
            c.set_char('│');
            c.set_style(border_style);
        }
        if let Some(c) = buf.cell_mut((right, y)) {
            c.set_char('│');
            c.set_style(border_style);
        }
    }

    let title = if is_failed {
        " OMP Shell Install Failed "
    } else {
        " Installing OMP Shell Host "
    };
    let title_style = Style::default()
        .fg(border_fg)
        .bg(bg)
        .add_modifier(Modifier::BOLD);
    buf.set_string(dialog.x + 2, dialog.y, title, title_style);

    let inner_x = dialog.x + 2;
    let inner_w = dialog.width.saturating_sub(4) as usize;
    let tick = (state.started_at.elapsed().as_millis() / 80) as usize;
    let spinner = SPINNER_FRAMES[tick % SPINNER_FRAMES.len()];

    // Row 1: status headline
    let (status_text, status_style) = match &phase {
        OmpShellInstallPhase::Resolving => (
            format!("{spinner} Resolving omp-shell-host-latest release..."),
            Style::default().fg(theme.text_primary).bg(bg),
        ),
        OmpShellInstallPhase::Downloading { asset_name, .. } => (
            format!("{spinner} Downloading {asset_name}"),
            Style::default().fg(theme.text_primary).bg(bg),
        ),
        OmpShellInstallPhase::Extracting => (
            format!("{spinner} Extracting grok-pi-shell-host..."),
            Style::default().fg(theme.text_primary).bg(bg),
        ),
        OmpShellInstallPhase::Verifying => (
            format!("{spinner} Verifying grok-pi-shell-host --handshake..."),
            Style::default().fg(theme.text_primary).bg(bg),
        ),
        OmpShellInstallPhase::Done(_) => (
            "✓ Installed & verified grok-pi-shell-host".to_string(),
            Style::default().fg(theme.accent_success).bg(bg),
        ),
        OmpShellInstallPhase::Failed(_) => (
            "✗ Could not install grok-pi-shell-host (OMP Shell stays off)".to_string(),
            Style::default()
                .fg(theme.accent_error)
                .bg(bg)
                .add_modifier(Modifier::BOLD),
        ),
        OmpShellInstallPhase::Cancelled => (
            "Cancelled (OMP Shell stays off)".to_string(),
            Style::default().fg(theme.gray_dim).bg(bg),
        ),
    };
    let status_line = truncate_to_width(&status_text, inner_w);
    buf.set_string(inner_x, dialog.y + 2, &status_line, status_style);

    // Row 2-3: progress bar OR wrapped error message
    match &phase {
        OmpShellInstallPhase::Failed(err) => {
            let wrapped = word_wrap(err, inner_w);
            let err_style = Style::default().fg(theme.text_secondary).bg(bg);
            for (idx, line) in wrapped.iter().take(2).enumerate() {
                let clipped = truncate_to_width(line, inner_w);
                buf.set_string(inner_x, dialog.y + 4 + idx as u16, &clipped, err_style);
            }
        }
        _ => {
            let (fraction, detail) = match &phase {
                OmpShellInstallPhase::Resolving => {
                    let pulse = ((tick % 20) as f32 / 20.0).clamp(0.05, 0.95);
                    (pulse, "resolving".to_string())
                }
                OmpShellInstallPhase::Downloading {
                    downloaded,
                    total: Some(total),
                    ..
                } if *total > 0 => {
                    let ratio = (*downloaded as f32 / *total as f32).clamp(0.0, 1.0);
                    let pct = (ratio * 100.0).round() as u32;
                    (
                        ratio,
                        format!("{pct:>3}%  {} / {}", format_bytes(*downloaded), format_bytes(*total)),
                    )
                }
                OmpShellInstallPhase::Downloading { downloaded, .. } => {
                    let pulse = ((tick % 20) as f32 / 20.0).clamp(0.05, 0.95);
                    (pulse, format_bytes(*downloaded))
                }
                OmpShellInstallPhase::Extracting => (0.90, " 90%  extracting".to_string()),
                OmpShellInstallPhase::Verifying => (0.97, " 97%  verifying".to_string()),
                OmpShellInstallPhase::Done(_) => (1.0, "100%  ready".to_string()),
                OmpShellInstallPhase::Cancelled | OmpShellInstallPhase::Failed(_) => {
                    (0.0, String::new())
                }
            };

            let detail_w = UnicodeWidthStr::width(detail.as_str());
            let bar_w = inner_w.saturating_sub(detail_w + 2).max(10) as u16;
            let bar_y = dialog.y + 4;
            render_progress_bar(
                buf,
                inner_x,
                bar_y,
                bar_w,
                fraction,
                theme.accent_tool,
                theme.bg_highlight,
            );
            let detail_x = inner_x + bar_w + 2;
            if (detail_x as usize) < (inner_x as usize + inner_w) {
                let remaining = (inner_x as usize + inner_w).saturating_sub(detail_x as usize);
                let clipped = truncate_to_width(&detail, remaining);
                buf.set_string(
                    detail_x,
                    bar_y,
                    &clipped,
                    Style::default().fg(theme.text_secondary).bg(bg),
                );
            }
        }
    }

    // Target path hint
    let dest = omp_shell_install_path()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "~/.grok-pi/bin/grok-pi-shell-host".to_string());
    let dest_line = truncate_to_width(&format!("Target: {dest}"), inner_w);
    buf.set_string(
        inner_x,
        dialog.y + dialog_height - 3,
        &dest_line,
        Style::default().fg(theme.gray_dim).bg(bg),
    );

    // Footer hints
    let footer = if is_failed {
        "Enter retry  ·  Esc close"
    } else {
        "Esc cancel"
    };
    let footer_w = UnicodeWidthStr::width(footer) as u16;
    let footer_x = dialog.x + dialog.width.saturating_sub(footer_w) / 2;
    buf.set_string(
        footer_x,
        dialog.y + dialog_height - 2,
        footer,
        Style::default().fg(theme.gray_dim).bg(bg),
    );
}
