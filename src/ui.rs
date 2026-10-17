use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
};

use crate::{
    app::{ACTIONS, Action, App, Form, FormKind, Mode},
    game_controller::Snapshot,
    model::{EventKind, MatchType, Session},
    teams::{team_name, team_number},
};

pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    frame.render_widget(
        Block::default().style(Style::default().bg(Color::Black).fg(Color::Gray)),
        area,
    );
    if area.width < 60 || area.height < 16 {
        frame.render_widget(Paragraph::new("Please resize to at least 60 columns × 16 rows.\nYour notes are preserved; resize to see the controls.").wrap(Wrap { trim: false }), area);
        return;
    }
    let rows = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(8),
        Constraint::Length(4),
    ])
    .split(area);
    let columns =
        Layout::horizontal([Constraint::Ratio(2, 3), Constraint::Ratio(1, 3)]).split(rows[1]);
    let (label, color) = app.mode_style();
    frame.render_widget(
        Paragraph::new(format!(" ● {label} ")).style(
            Style::default()
                .fg(Color::Black)
                .bg(color)
                .add_modifier(Modifier::BOLD),
        ),
        Rect {
            height: 1,
            ..rows[0]
        },
    );
    frame.render_widget(
        Paragraph::new(match_status(app)),
        Rect {
            y: rows[0].y + 1,
            height: 1,
            ..rows[0]
        },
    );
    frame.render_widget(
        Paragraph::new(controller_status(app)),
        Rect {
            y: rows[0].y + 2,
            height: 1,
            ..rows[0]
        },
    );
    draw_preview(frame, app, columns[1], color);
    match &mut app.mode {
        Mode::Form(form) => draw_form(frame, form, columns[0], color),
        Mode::Actions {
            selected,
            timestamp,
            gc_seconds_remaining,
            ..
        } => {
            let match_type = app.session.as_ref().and_then(|session| session.match_type);
            let combined = match_type == Some(MatchType::Combined);
            let golden_goal = app
                .session
                .as_ref()
                .is_some_and(|session| session.in_golden_goal());
            let finished = match_type.is_some()
                && app
                    .session
                    .as_ref()
                    .is_some_and(|session| session.is_finished());
            let automatic_goals = app.controller.is_some()
                && app.session.as_ref().is_some_and(|session| {
                    session.match_type.is_some() || !session.in_golden_goal()
                });
            let items: Vec<ListItem> = ACTIONS
                .iter()
                .map(|(shortcut, action)| {
                    let label = match action {
                        Action::TrueData
                            if app
                                .controller
                                .as_ref()
                                .is_some_and(|controller| controller.true_data) =>
                        {
                            "Use GC broadcasts (stop true-data requests)".into()
                        }
                        Action::Mark(kind @ (EventKind::GoalHome | EventKind::GoalAway)) => {
                            app.session.as_ref().map_or_else(
                                || action.label().to_owned(),
                                |session| {
                                    let (home, away) = session.current_teams();
                                    format!(
                                        "Goal — {}",
                                        if *kind == EventKind::GoalHome {
                                            home
                                        } else {
                                            away
                                        }
                                    )
                                },
                            )
                        }
                        Action::Mark(EventKind::GameStart)
                            if combined && golden_goal && finished =>
                        {
                            if app.controller.is_some() {
                                "Waiting for GC opponent change".into()
                            } else {
                                "Start official game (manual)".into()
                            }
                        }
                        Action::Mark(EventKind::GameStart | EventKind::GoldenGoalStart)
                            if match_type == Some(MatchType::GoldenGoal)
                                || (combined && golden_goal) =>
                        {
                            "Golden Goal start".into()
                        }
                        Action::Mark(EventKind::GameEnd) if combined => {
                            if golden_goal {
                                "Golden Goal end".into()
                            } else {
                                "Official game end".into()
                            }
                        }
                        Action::Controller if app.controller.is_some() => {
                            if finished {
                                "Stop GameController".into()
                            } else {
                                "Stop GC; enable manual goals".into()
                            }
                        }
                        _ => action.label().to_owned(),
                    };
                    let (label, disabled) = match action {
                        Action::Mark(EventKind::GameStart) if combined && golden_goal => {
                            (label, finished && app.controller.is_some())
                        }
                        Action::Mark(kind)
                            if finished && !matches!(kind, EventKind::Note | EventKind::Custom) =>
                        {
                            (format!("Unavailable: finished — {label}"), true)
                        }
                        Action::Mark(EventKind::HalfTime | EventKind::SecondHalfStart)
                            if golden_goal && match_type.is_some() =>
                        {
                            (format!("Unavailable: Golden Goal — {label}"), true)
                        }
                        Action::Mark(EventKind::GoldenGoalStart)
                            if match_type == Some(MatchType::Official)
                                || (combined && !golden_goal) =>
                        {
                            ("Unavailable: new session needed".into(), true)
                        }
                        Action::Mark(EventKind::GoalHome | EventKind::GoalAway)
                            if automatic_goals =>
                        {
                            (format!("Automatic: GC — {label}"), true)
                        }
                        _ => (label, false),
                    };
                    ListItem::new(format!("{shortcut}  {label}")).style(if disabled {
                        Style::default().fg(Color::DarkGray)
                    } else {
                        Style::default()
                    })
                })
                .collect();
            let list = List::new(items)
                .block(
                    panel(
                        format!("Actions @ {}", timestamp.format("%H:%M:%S%.3f %:z")),
                        color,
                    )
                    .title_bottom(format!(
                        "GC {} — captured on Tab",
                        gc_seconds_remaining
                            .map(crate::game_controller::format_clock)
                            .unwrap_or_else(|| "unavailable".into())
                    )),
                )
                .highlight_style(
                    Style::default()
                        .bg(color)
                        .fg(Color::Black)
                        .add_modifier(Modifier::BOLD),
                )
                .highlight_symbol("▶ ");
            frame.render_stateful_widget(
                list,
                columns[0],
                &mut ListState::default().with_selected(Some(*selected)),
            );
        }
        Mode::Delete(index) => {
            let description = app
                .session
                .as_ref()
                .map(|session| {
                    let entry = &session.entries[*index];
                    format!(
                        "{}  {}\n{}",
                        entry.timestamp.format("%H:%M:%S%.3f"),
                        entry.kind.label(),
                        entry.text
                    )
                })
                .unwrap_or_default();
            frame.render_widget(Paragraph::new(format!("Remove this event?\n\n{description}\n\nY: mark as undone\nN / Esc: keep it active\n\nStill saved. Tab, B restores it.\nThe score will be recalculated.")).wrap(Wrap { trim: false }).block(panel("Reversible removal", color)), columns[0]);
        }
        _ => draw_log(frame, app, columns[0], color),
    }
    draw_footer(frame, app, rows[2]);
}

fn panel(title: impl Into<Line<'static>>, color: Color) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(color))
        .title(title)
}

fn opponent_warning(session: &Session, packet: &Snapshot) -> Option<(String, Color)> {
    // The receiver can already show the new opponent while Tab defers its import.
    let pending_official = session.match_type == Some(MatchType::Combined)
        && session.in_golden_goal()
        && session.is_finished()
        && session
            .golden_goal_opponent
            .is_some_and(|opponent| opponent != packet.opponent_number);
    let (home, away) = if pending_official {
        (
            session.metadata.home.as_str(),
            session.metadata.away.as_str(),
        )
    } else {
        session.current_teams()
    };
    let configured = if session.metadata.hulks_are_home() {
        away
    } else {
        home
    };
    let received = packet.opponent_number;
    let received_name = team_name(received).unwrap_or("unknown team");
    match team_number(configured) {
        Some(expected) if expected == received => None,
        Some(expected) => Some((
            format!(
                "OPPONENT MISMATCH: set #{expected} {configured}; GC #{received} {received_name}"
            ),
            Color::LightRed,
        )),
        None => Some((
            format!("OPPONENT UNVERIFIED: {configured}; GC #{received} {received_name}"),
            Color::LightYellow,
        )),
    }
}

fn controller_status(app: &App) -> Line<'static> {
    if let Some(error) = &app.controller_error {
        return Line::styled(
            format!(" GC ERROR: {error}"),
            Style::default().fg(Color::LightRed),
        );
    }
    let Some(controller) = &app.controller else {
        return Line::styled(
            if app.session.as_ref().is_some_and(|session| {
                session.match_type == Some(MatchType::Combined)
                    && session.in_golden_goal()
                    && session.is_finished()
            }) {
                " GC OFF • waiting for official opponent • Tab, g: switch manually"
            } else if app
                .session
                .as_ref()
                .is_some_and(|session| session.match_type.is_some() && session.is_finished())
            {
                " GC OFF • match finished • notes/corrections available"
            } else {
                " GC OFF • Tab, l: listen (v20) • manual goals enabled"
            },
            Style::default().fg(Color::DarkGray),
        );
    };
    let (label, color) = if controller.latest.is_none() {
        ("WAITING", Color::LightYellow)
    } else if controller.is_stale() {
        ("STALE", Color::LightRed)
    } else {
        ("LIVE", Color::LightGreen)
    };
    let detail = match (&controller.latest, &app.session) {
        (Some(packet), Some(session)) => {
            let (home, away) = if session.metadata.hulks_are_home() {
                (packet.hulks_score, packet.opponent_score)
            } else {
                (packet.opponent_score, packet.hulks_score)
            };
            let (home_team, away_team) = if session.match_type == Some(MatchType::Combined) {
                session.current_teams()
            } else {
                (
                    session.metadata.home.as_str(),
                    session.metadata.away.as_str(),
                )
            };
            format!(
                " {} • {home_team} {home}:{away} {away_team} • {}",
                packet.clock(),
                packet.summary()
            )
        }
        _ => {
            if app
                .session
                .as_ref()
                .is_some_and(|session| session.match_type.is_none() && session.in_golden_goal())
            {
                " UDP 3838 • manual Golden Goal goals enabled".into()
            } else {
                " UDP 3838 • goal keys locked; Tab, l stops".into()
            }
        }
    };
    let detail = controller
        .notice
        .as_ref()
        .map_or(detail, |notice| format!(" {notice}"));
    let warning = app
        .session
        .as_ref()
        .zip(controller.latest.as_ref())
        .and_then(|(session, packet)| opponent_warning(session, packet));
    let (detail, color) = match warning {
        Some((warning, warning_color)) => (format!(" {warning} •{detail}"), warning_color),
        None => (detail, color),
    };
    Line::from(vec![
        Span::styled(
            format!(
                " GC {}{label}{} ",
                if controller.true_data { "TRUE " } else { "" },
                if app.pending_controller.is_empty() {
                    String::new()
                } else {
                    format!(" ({} queued)", app.pending_controller.len())
                }
            ),
            Style::default()
                .fg(Color::Black)
                .bg(color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(detail, Style::default().fg(color)),
    ])
}

fn match_status(app: &App) -> Line<'static> {
    if let Mode::Form(form) = &app.mode
        && !matches!(form.kind, FormKind::Entry(_))
    {
        let home = form.fields[1].value().trim().eq_ignore_ascii_case("HULKs");
        let away = form.fields[2].value().trim().eq_ignore_ascii_case("HULKs");
        let match_type = match form.match_type {
            Some(MatchType::Combined) => "COMBINED • ",
            Some(MatchType::Official) => "OFFICIAL • ",
            Some(MatchType::GoldenGoal) => "GOLDEN GOAL • ",
            None => "",
        };
        return if home == away {
            Line::styled(
                format!(" {match_type}Team check: exactly one team must be HULKs"),
                Style::default().fg(Color::LightRed),
            )
        } else {
            Line::styled(
                format!(
                    " {match_type}HULKs: {}{}",
                    if home { "HOME" } else { "AWAY" },
                    match form.match_type {
                        Some(MatchType::GoldenGoal) => " • Opponent: Invisibles",
                        Some(MatchType::Combined | MatchType::Official) => "",
                        None => " • Golden Goal opponent: Invisibles",
                    }
                ),
                Style::default().fg(Color::LightGreen),
            )
        };
    }
    let Some(session) = &app.session else {
        return Line::from(" New match • timestamps use this laptop’s wall clock and timezone");
    };
    let combined = session.match_type == Some(MatchType::Combined);
    let test = session.in_golden_goal();
    let phase = if test && session.match_type.is_some() {
        if session.is_finished() {
            if combined {
                (
                    "GOLDEN GOAL • waiting for official opponent",
                    Color::LightYellow,
                )
            } else {
                ("GOLDEN GOAL • FINISHED", Color::Gray)
            }
        } else {
            ("GOLDEN GOAL", Color::LightMagenta)
        }
    } else if session.match_type.is_some() && session.is_finished() {
        ("FULL TIME", Color::Gray)
    } else if test {
        ("GOLDEN GOAL TEST", Color::LightMagenta)
    } else {
        session
            .entries
            .iter()
            .rev()
            .filter(|entry| {
                !entry.undone && (!combined || entry.game == Some(session.current_game()))
            })
            .find_map(|entry| match entry.kind {
                EventKind::GameStart => Some(("FIRST HALF", Color::LightGreen)),
                EventKind::HalfTime => Some(("HALFTIME", Color::LightYellow)),
                EventKind::SecondHalfStart => Some(("SECOND HALF", Color::LightGreen)),
                EventKind::GameEnd => Some(("FULL TIME", Color::Gray)),
                _ => None,
            })
            .unwrap_or(("PRE-GAME", Color::LightCyan))
    };
    let (home, away) = if test {
        session.golden_goal_score()
    } else {
        session.score()
    };
    let (home_team, away_team) = session.current_teams();
    let detail = if test && session.match_type != Some(MatchType::GoldenGoal) {
        let (home, away) = session.score();
        format!(
            "{}: {} {home}:{away} {}",
            if combined { "official" } else { "match" },
            session.metadata.home,
            session.metadata.away
        )
    } else if combined {
        let (home, away) = session.golden_goal_score();
        let (home_team, away_team) = if session.metadata.hulks_are_home() {
            (session.metadata.home.as_str(), "Invisibles")
        } else {
            ("Invisibles", session.metadata.away.as_str())
        };
        format!("Golden Goal: {home_team} {home}:{away} {away_team}")
    } else {
        session.metadata.event.clone()
    };
    Line::from(vec![
        Span::styled(
            format!(
                " {}{} ",
                if session.match_type.is_some() && !test {
                    "OFFICIAL • "
                } else {
                    ""
                },
                phase.0
            ),
            Style::default()
                .fg(Color::Black)
                .bg(phase.1)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  {home_team} {home}:{away} {away_team}"),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!(
            " • HULKs: {} • {detail}",
            if session.metadata.hulks_are_home() {
                "home"
            } else {
                "away"
            }
        )),
    ])
}

fn draw_form(frame: &mut Frame, form: &mut Form, area: Rect, color: Color) {
    let title = match form.kind {
        FormKind::Setup => "Match setup — select Create",
        FormKind::Metadata => "Match details — select Apply",
        FormKind::Entry(_) => "Correct event — select Apply",
    };
    let block = panel(title, color);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let sections = Layout::vertical([Constraint::Min(3), Constraint::Length(1)]).split(inner);
    let fields_area = sections[0];
    let count = (fields_area.height as usize / 3)
        .max(1)
        .min(form.fields.len());
    let start = form
        .selected
        .min(form.fields.len().saturating_sub(1))
        .saturating_sub(count.saturating_sub(1));
    for (visible, field) in form.fields.iter_mut().skip(start).take(count).enumerate() {
        let selected = visible + start == form.selected;
        let field_area = Rect {
            x: fields_area.x,
            y: fields_area.y + visible as u16 * 3,
            width: fields_area.width,
            height: 3.min(fields_area.height.saturating_sub(visible as u16 * 3)),
        };
        field.editor.set_block(panel(
            format!("{} {}", if selected { "▶" } else { " " }, field.label),
            if selected { color } else { Color::DarkGray },
        ));
        field.editor.set_style(Style::default().fg(if selected {
            Color::White
        } else {
            Color::Gray
        }));
        field.editor.set_cursor_line_style(Style::default());
        field.editor.set_cursor_style(if selected {
            Style::default().bg(color).fg(Color::Black)
        } else {
            Style::default()
        });
        frame.render_widget(&field.editor, field_area);
    }
    let selected = form.selected == form.fields.len();
    let label = if matches!(form.kind, FormKind::Setup) {
        "Create match"
    } else {
        "Apply"
    };
    frame.render_widget(
        Paragraph::new(format!(
            "{} [ {label} ]{}",
            if selected { "▶" } else { " " },
            if selected { "  Enter" } else { "" },
        ))
        .style(if selected {
            Style::default()
                .bg(color)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(color)
        }),
        sections[1],
    );
}

fn draw_log(frame: &mut Frame, app: &mut App, area: Rect, color: Color) {
    let sections = Layout::vertical([Constraint::Min(3), Constraint::Length(4)]).split(area);
    let items: Vec<ListItem> = app
        .session
        .as_ref()
        .map(|session| {
            session
                .entries
                .iter()
                .enumerate()
                .map(|(index, entry)| {
                    let color = match entry.kind {
                        EventKind::Note => Color::Gray,
                        EventKind::GoalHome | EventKind::GoalAway => Color::LightYellow,
                        EventKind::HalfTime => Color::LightBlue,
                        _ => Color::LightMagenta,
                    };
                    let mut lines = vec![Line::from(vec![
                        Span::styled(
                            format!("{} ", entry.timestamp.format("%H:%M:%S%.3f")),
                            Style::default().fg(Color::DarkGray),
                        ),
                        Span::styled(
                            format!(
                                "{}{}{}",
                                if entry.undone { "[undone] " } else { "" },
                                match (session.match_type, entry.game) {
                                    (Some(MatchType::Combined), Some(MatchType::GoldenGoal)) => {
                                        "[Golden Goal] "
                                    }
                                    (Some(MatchType::Combined), Some(MatchType::Official)) => {
                                        "[Official] "
                                    }
                                    _ => "",
                                },
                                entry.kind.label()
                            ),
                            Style::default().fg(color).add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            if session.active_note == Some(index) {
                                "  [active note]"
                            } else {
                                ""
                            },
                            Style::default().fg(Color::LightGreen),
                        ),
                    ])];
                    if !entry.text.is_empty() {
                        lines.push(Line::from(format!("  {}", entry.text)));
                    }
                    ListItem::new(Text::from(lines)).style(if entry.undone {
                        Style::default().add_modifier(Modifier::DIM)
                    } else {
                        Style::default()
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let list = List::new(items)
        .block(panel("Events — ↑/↓ idle • Tab e correct", color))
        .highlight_style(Style::default().bg(Color::DarkGray).fg(Color::White))
        .highlight_symbol("▶ ");
    frame.render_stateful_widget(list, sections[0], &mut app.history);
    if matches!(app.mode, Mode::Writing) {
        let timestamp = app
            .session
            .as_ref()
            .and_then(|session| {
                session.active_note.map(|index| {
                    session.entries[index]
                        .timestamp
                        .format("%H:%M:%S%.3f %:z")
                        .to_string()
                })
            })
            .unwrap_or_default();
        app.note_editor
            .set_block(panel(format!("Note @ {timestamp} — Enter submits"), color));
        app.note_editor.set_style(Style::default().fg(Color::White));
        app.note_editor
            .set_cursor_style(Style::default().fg(Color::Black).bg(color));
        app.note_editor.set_cursor_line_style(Style::default());
        frame.render_widget(&app.note_editor, sections[1]);
    } else {
        frame.render_widget(Paragraph::new("Enter: mark the moment, then describe it.\nNo next-note timestamp is running while idle.").block(panel("Ready", color)), sections[1]);
    }
}

fn draw_preview(frame: &mut Frame, app: &mut App, area: Rect, color: Color) {
    let text = app.preview();
    let lines: Vec<Line> = text
        .lines()
        .map(|line| {
            let style = if line.starts_with('#') {
                Style::default()
                    .fg(Color::LightCyan)
                    .add_modifier(Modifier::BOLD)
            } else if line.starts_with("Goal ") {
                Style::default().fg(Color::LightYellow)
            } else {
                Style::default().fg(Color::Gray)
            };
            Line::styled(line.to_owned(), style)
        })
        .collect();
    let title = if matches!(app.mode, Mode::Form(_)) {
        "Markdown preview (form not applied)"
    } else {
        "Markdown preview"
    };
    let block = panel(title, color);
    let inner = block.inner(area);
    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
    let max_scroll = paragraph
        .line_count(inner.width)
        .saturating_sub(inner.height as usize)
        .min(u16::MAX as usize) as u16;
    app.preview_max = max_scroll;
    let scroll = app.preview_scroll.unwrap_or(max_scroll).min(max_scroll);
    frame.render_widget(block, area);
    frame.render_widget(paragraph.scroll((scroll, 0)), inner);
}

fn draw_footer(frame: &mut Frame, app: &App, area: Rect) {
    let (help, extra) = match &app.mode {
        Mode::Form(form) => (
            if matches!(form.kind, FormKind::Setup) {
                "Tab/↑/↓: select  Enter: next/create  Esc: quit"
            } else {
                "Tab/↑/↓: select  Enter: next/apply  Esc: cancel"
            },
            if matches!(form.kind, FormKind::Setup) {
                "Match type: ←/→ changes; fixed after Create."
            } else {
                "Select the button, then Enter; edits apply together."
            },
        ),
        Mode::Actions { .. } => (
            "Shortcut/Enter: choose  ↑/↓: select  Esc: cancel",
            "Event time captured on Tab • [/] preview • w save • q quit",
        ),
        Mode::Delete(_) => (
            "y: remove  n/Esc: cancel",
            "Saved as undone; select it and Tab b to restore.",
        ),
        _ => (
            "Enter: start/submit note  Tab: actions",
            match app.session.as_ref().and_then(|session| session.match_type) {
                Some(MatchType::Combined)
                    if app
                        .session
                        .as_ref()
                        .is_some_and(|session| session.in_golden_goal()) =>
                {
                    if app
                        .session
                        .as_ref()
                        .is_some_and(|session| session.is_finished())
                    {
                        if app.controller.is_some() {
                            "Waiting for official opponent • Tab, l: stop GC for manual switch"
                        } else {
                            "Waiting for official opponent • Tab, g: switch manually"
                        }
                    } else {
                        "Tab then: g/t Golden Goal start  f end test  1/2 goal  q quit"
                    }
                }
                Some(_)
                    if app
                        .session
                        .as_ref()
                        .is_some_and(|session| session.is_finished()) =>
                {
                    "Finished • Tab: e correct  d remove  n note  q quit"
                }
                Some(MatchType::GoldenGoal) => {
                    "Tab then: g/t Golden Goal start  1/2 goal  e correct  q quit"
                }
                Some(MatchType::Combined | MatchType::Official) => {
                    "Tab then: g start  h half-time  1/2 goal  e correct  q quit"
                }
                None => "Tab then: t test  1/2 goal  g start  e correct  q quit",
            },
        ),
    };
    let path = app
        .store
        .as_ref()
        .map(|store| {
            format!(
                "{} {}",
                if !app.pending_controller.is_empty() {
                    "GC QUEUED (unsaved)"
                } else if app.dirty {
                    "UNSAVED"
                } else {
                    "SAVED"
                },
                store.markdown_path().display()
            )
        })
        .unwrap_or_else(|| "No file created yet".into());
    let (message, color) = if let Some(error) = &app.save_error {
        (error.as_str(), Color::LightRed)
    } else {
        (app.status.as_str(), Color::LightYellow)
    };
    let lines = vec![
        Line::styled(help, Style::default().fg(Color::White)),
        Line::from(extra),
        Line::styled(message, Style::default().fg(color)),
        Line::styled(
            path,
            Style::default().fg(if app.dirty || !app.pending_controller.is_empty() {
                Color::LightRed
            } else {
                Color::DarkGray
            }),
        ),
    ];
    frame.render_widget(Paragraph::new(lines), area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    fn captured_time() -> chrono::DateTime<chrono::FixedOffset> {
        chrono::DateTime::parse_from_rfc3339("2026-10-03T14:32:08.123+02:00").unwrap()
    }

    #[test]
    fn setup_has_a_colored_mode_banner_and_right_third_preview() {
        let mut app = App::new(None).unwrap();
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(0, 0)].bg, Color::LightBlue);
        let text: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
        assert!(text.contains("MATCH SETUP"));
        assert!(text.contains("Markdown preview"));
        assert_eq!(buffer[(80, 3)].symbol(), "┌");
    }

    #[test]
    fn setup_shows_fixed_match_type_and_arrow_selection_at_minimum_size() {
        let mut app = App::new(None).unwrap();
        let Mode::Form(form) = &mut app.mode else {
            panic!("expected setup form");
        };
        assert_eq!(form.match_type, Some(MatchType::Combined));
        assert_eq!(form.fields.len(), 12);
        assert_eq!(form.fields[10].label, "Output directory");
        form.selected = 11;
        let mut terminal = Terminal::new(TestBackend::new(60, 16)).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        for expected in [
            "COMBINED",
            "Match type",
            "Golden Goal + official",
            "fixed after Create",
        ] {
            assert!(text.contains(expected), "Missing {expected}");
        }

        app.key(
            crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Right,
                crossterm::event::KeyModifiers::NONE,
            ),
            captured_time(),
        )
        .unwrap();
        let Mode::Form(form) = &app.mode else {
            unreachable!();
        };
        assert_eq!(form.match_type, Some(MatchType::GoldenGoal));
        assert_eq!(form.fields[2].value(), "Invisibles");
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        for expected in ["GOLDEN GOAL", "Golden Goal", "Opponent: Invisibles"] {
            assert!(text.contains(expected), "Missing {expected}");
        }
        app.key(
            crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Right,
                crossterm::event::KeyModifiers::NONE,
            ),
            captured_time(),
        )
        .unwrap();
        let Mode::Form(form) = &app.mode else {
            unreachable!();
        };
        assert_eq!(form.match_type, Some(MatchType::Official));
    }

    #[test]
    fn form_buttons_stay_visible_and_highlight_when_selected_at_minimum_size() {
        for (kind, label, color) in [
            (FormKind::Setup, "Create match", Color::LightBlue),
            (FormKind::Metadata, "Apply", Color::LightYellow),
        ] {
            let mut app = App::new(None).unwrap();
            let Mode::Form(form) = &mut app.mode else {
                panic!("expected setup form");
            };
            form.kind = kind;
            if matches!(kind, FormKind::Metadata) {
                form.fields.truncate(10);
            }
            let mut terminal = Terminal::new(TestBackend::new(60, 16)).unwrap();
            terminal.draw(|frame| draw(frame, &mut app)).unwrap();
            let text: String = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect();
            assert!(text.contains(&format!("[ {label} ]")));
            assert_ne!(terminal.backend().buffer()[(1, 10)].bg, color);

            let Mode::Form(form) = &mut app.mode else {
                unreachable!();
            };
            form.selected = form.fields.len();
            terminal.draw(|frame| draw(frame, &mut app)).unwrap();
            let buffer = terminal.backend().buffer();
            let text: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
            assert!(text.contains(&format!("▶ [ {label} ]  Enter")));
            assert_eq!(buffer[(1, 10)].bg, color);
            assert_eq!(buffer[(40, 3)].symbol(), "┌");
            assert!(text.contains("Markdown preview"));
        }
    }

    #[test]
    fn action_menu_shows_shortcuts_and_scrolls_selection_at_minimum_size() {
        let mut app = App::new(None).unwrap();
        app.mode = Mode::Actions {
            selected: 0,
            timestamp: captured_time(),
            gc_seconds_remaining: None,
            message_budget: None,
        };
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        for (shortcut, action) in ACTIONS {
            assert!(text.contains(&format!("{shortcut}  {}", action.label())));
        }
        assert!(text.contains("14:32:08.123 +02:00"));

        let mut terminal = Terminal::new(TestBackend::new(60, 16)).unwrap();
        for (selected, (shortcut, _)) in ACTIONS.iter().enumerate() {
            app.mode = Mode::Actions {
                selected,
                timestamp: captured_time(),
                gc_seconds_remaining: None,
                message_budget: None,
            };
            terminal.draw(|frame| draw(frame, &mut app)).unwrap();
            let buffer = terminal.backend().buffer();
            let text: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
            assert!(text.contains(&format!("▶ {shortcut}  ")));
            assert!(text.contains("ACTION MENU"));
            assert_eq!(buffer[(40, 3)].symbol(), "┌");
        }
    }

    #[test]
    fn golden_goal_menu_uses_invisibles_and_keeps_hulks_on_the_away_side() {
        let mut app = App::new(None).unwrap();
        let metadata = crate::model::Metadata {
            event: "Hobbymesse".into(),
            home: "HTWK".into(),
            away: "HULKs".into(),
            date: captured_time().date_naive(),
            label: "1st_game".into(),
            robots: std::array::from_fn(|_| String::new()),
        };
        let mut session = crate::model::Session::new(metadata);
        session.record(EventKind::GoldenGoalStart, captured_time());
        session.record(EventKind::GoalAway, captured_time());
        app.session = Some(session);
        app.mode = Mode::Actions {
            selected: 0,
            timestamp: captured_time(),
            gc_seconds_remaining: None,
            message_budget: None,
        };
        let mut terminal = Terminal::new(TestBackend::new(140, 40)).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        for expected in [
            "GOLDEN GOAL TEST",
            "Invisibles 0:1 HULKs",
            "HULKs: away",
            "match: HTWK 0:0 HULKs",
            "1  Goal — Invisibles",
            "2  Goal — HULKs",
        ] {
            assert!(text.contains(expected), "Missing {expected}");
        }
    }

    fn combined_app() -> App {
        let mut app = App::new(None).unwrap();
        app.session = Some(crate::model::Session::new_match(
            crate::model::Metadata {
                event: "Hobbymesse".into(),
                home: "HULKs".into(),
                away: "HTWK".into(),
                date: captured_time().date_naive(),
                label: "game".into(),
                robots: std::array::from_fn(|_| String::new()),
            },
            MatchType::Combined,
        ));
        app.mode = Mode::Idle;
        app
    }

    fn gc_snapshot(opponent_number: u8) -> Snapshot {
        Snapshot {
            packet_number: 1,
            game_phase: 0,
            state: 3,
            set_play: 0,
            first_half: true,
            stopped: false,
            kicking_team: 24,
            seconds_remaining: 590,
            secondary_time: 0,
            hulks_score: 0,
            hulks_message_budget: 12000,
            hulks_penalties: [0; 20],
            opponent_score: 0,
            opponent_number,
        }
    }

    #[test]
    fn opponent_warning_checks_names_numbers_and_both_hulks_sides() {
        for hulks_home in [true, false] {
            let mut app = combined_app();
            let session = app.session.as_mut().unwrap();
            session.match_type = Some(MatchType::Official);
            if !hulks_home {
                std::mem::swap(&mut session.metadata.home, &mut session.metadata.away);
            }
            assert!(opponent_warning(session, &gc_snapshot(14)).is_none());
            let (warning, color) = opponent_warning(session, &gc_snapshot(5)).unwrap();
            assert!(warning.contains("OPPONENT MISMATCH"));
            assert!(warning.contains("#14 HTWK"));
            assert!(warning.contains("#5 B-Human"));
            assert_eq!(color, Color::LightRed);
            let opponent = if hulks_home {
                &mut session.metadata.away
            } else {
                &mut session.metadata.home
            };
            *opponent = "Unlisted team".into();
            let (warning, color) = opponent_warning(session, &gc_snapshot(14)).unwrap();
            assert!(warning.contains("OPPONENT UNVERIFIED"));
            assert_eq!(color, Color::LightYellow);
        }
    }

    #[test]
    fn golden_opponent_warning_uses_invisibles_and_handles_queued_official_transition() {
        let mut app = combined_app();
        let session = app.session.as_mut().unwrap();
        assert!(opponent_warning(session, &gc_snapshot(0)).is_none());
        assert!(
            opponent_warning(session, &gc_snapshot(14))
                .unwrap()
                .0
                .contains("#0 Invisibles")
        );
        session.record(EventKind::GameEnd, captured_time());
        session.golden_goal_opponent = Some(0);
        assert!(opponent_warning(session, &gc_snapshot(14)).is_none());
        assert!(
            opponent_warning(session, &gc_snapshot(5))
                .unwrap()
                .0
                .contains("#14 HTWK")
        );
        // Standalone Golden Goal never automatically transitions to the official opponent.
        session.match_type = Some(MatchType::GoldenGoal);
        session.metadata.away = "Invisibles".into();
        assert!(
            opponent_warning(session, &gc_snapshot(14))
                .unwrap()
                .0
                .contains("#0 Invisibles")
        );
    }

    #[test]
    fn opponent_warning_stays_visible_over_notices_and_clears_after_metadata_fix() {
        let mut app = combined_app();
        app.session.as_mut().unwrap().match_type = Some(MatchType::Official);
        let mut receiver = crate::game_controller::Receiver::for_test();
        receiver.latest = Some(gc_snapshot(5));
        receiver.received_at = Some(std::time::Instant::now());
        receiver.notice = Some("Ignored unrelated sender".into());
        app.controller = Some(receiver);
        let original = app.session.as_ref().unwrap().clone();
        let line = controller_status(&app);
        assert_eq!(line.spans[0].style.bg, Some(Color::LightRed));
        assert!(line.to_string().contains("OPPONENT MISMATCH"));
        assert!(line.to_string().contains("Ignored unrelated sender"));
        assert_eq!(app.session.as_ref().unwrap(), &original);
        let mut terminal = Terminal::new(TestBackend::new(60, 16)).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("OPPONENT MISMATCH"));
        app.session.as_mut().unwrap().metadata.away = "B-Human".into();
        assert!(
            !controller_status(&app)
                .to_string()
                .contains("OPPONENT MISMATCH")
        );
        assert_eq!(
            controller_status(&app).spans[0].style.bg,
            Some(Color::LightGreen)
        );
    }

    #[test]
    fn true_data_status_and_action_are_explicit_without_silent_fallback() {
        let mut app = combined_app();
        let mut receiver = crate::game_controller::Receiver::for_test();
        receiver.true_data = true;
        app.controller = Some(receiver);
        assert!(
            controller_status(&app)
                .to_string()
                .contains("GC TRUE WAITING")
        );
        app.controller.as_mut().unwrap().latest = Some(gc_snapshot(0));
        app.controller.as_mut().unwrap().received_at = Some(std::time::Instant::now());
        assert!(controller_status(&app).to_string().contains("GC TRUE LIVE"));
        app.controller.as_mut().unwrap().received_at =
            Some(std::time::Instant::now() - std::time::Duration::from_secs(6));
        assert!(
            controller_status(&app)
                .to_string()
                .contains("GC TRUE STALE")
        );
        app.mode = Mode::Actions {
            selected: ACTIONS.iter().position(|(key, _)| *key == 'v').unwrap(),
            timestamp: captured_time(),
            message_budget: None,
            gc_seconds_remaining: None,
        };
        let mut terminal = Terminal::new(TestBackend::new(120, 35)).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("v  Use GC broadcasts"));
    }

    #[test]
    fn combined_waiting_keeps_test_score_and_official_opponent_on_either_side() {
        for hulks_home in [true, false] {
            let mut app = combined_app();
            let session = app.session.as_mut().unwrap();
            if !hulks_home {
                std::mem::swap(&mut session.metadata.home, &mut session.metadata.away);
            }
            session.record(EventKind::GoldenGoalStart, captured_time());
            session.record(
                if hulks_home {
                    EventKind::GoalHome
                } else {
                    EventKind::GoalAway
                },
                captured_time(),
            );
            session.record(EventKind::GameEnd, captured_time());
            let status = match_status(&app).to_string();
            assert!(status.contains("GOLDEN GOAL • waiting for official opponent"));
            assert!(!status.contains("FULL TIME"));
            assert!(status.contains(if hulks_home {
                "HULKs 1:0 Invisibles"
            } else {
                "Invisibles 0:1 HULKs"
            }));
            assert!(status.contains(if hulks_home {
                "official: HULKs 0:0 HTWK"
            } else {
                "official: HTWK 0:0 HULKs"
            }));
            assert!(
                controller_status(&app)
                    .to_string()
                    .contains("Tab, g: switch manually")
            );
        }
    }

    #[test]
    fn combined_official_phase_ignores_test_end_and_keeps_scores_scoped() {
        let mut app = combined_app();
        let session = app.session.as_mut().unwrap();
        session.record(EventKind::GoldenGoalStart, captured_time());
        session.record(EventKind::GoalHome, captured_time());
        session.record(EventKind::GameEnd, captured_time());
        session.record(EventKind::Custom, captured_time());
        session.entries.last_mut().unwrap().game = Some(MatchType::Official);
        let status = match_status(&app).to_string();
        assert!(status.contains("OFFICIAL • PRE-GAME"));
        assert!(!status.contains("FULL TIME"));
        assert!(!app.session.as_ref().unwrap().is_finished());
        app.session
            .as_mut()
            .unwrap()
            .record(EventKind::GoalAway, captured_time());
        for (kind, phase) in [
            (EventKind::GameStart, "FIRST HALF"),
            (EventKind::HalfTime, "HALFTIME"),
            (EventKind::SecondHalfStart, "SECOND HALF"),
            (EventKind::GameEnd, "FULL TIME"),
        ] {
            app.session.as_mut().unwrap().record(kind, captured_time());
            let status = match_status(&app).to_string();
            assert!(status.contains(&format!("OFFICIAL • {phase}")));
            assert!(status.contains("HULKs 0:1 HTWK"));
            assert!(status.contains("Golden Goal: HULKs 1:0 Invisibles"));
        }
        let mut terminal = Terminal::new(TestBackend::new(140, 40)).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("[Golden Goal] Game end"));
        assert!(text.contains("[Official] Game end"));
    }

    #[test]
    fn combined_action_menu_distinguishes_test_waiting_and_official() {
        let mut app = combined_app();
        let mut terminal = Terminal::new(TestBackend::new(140, 40)).unwrap();
        for stage in 0..3 {
            let session = app.session.as_mut().unwrap();
            if stage == 1 {
                session.record(EventKind::GameEnd, captured_time());
            } else if stage == 2 {
                session.record(EventKind::GameStart, captured_time());
                session.entries.last_mut().unwrap().game = Some(MatchType::Official);
            }
            app.mode = Mode::Actions {
                selected: 0,
                timestamp: captured_time(),
                gc_seconds_remaining: None,
                message_budget: None,
            };
            terminal.draw(|frame| draw(frame, &mut app)).unwrap();
            let text: String = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect();
            match stage {
                0 => {
                    assert!(text.contains("g  Golden Goal start"));
                    assert!(text.contains("t  Golden Goal start"));
                    assert!(text.contains("h  Unavailable: Golden Goal"));
                    assert!(text.contains("s  Unavailable: Golden Goal"));
                    assert!(text.contains("f  Golden Goal end"));
                    assert!(text.contains("2  Goal — Invisibles"));
                    app.controller = Some(crate::game_controller::Receiver::for_test());
                    terminal.draw(|frame| draw(frame, &mut app)).unwrap();
                    let text: String = terminal
                        .backend()
                        .buffer()
                        .content
                        .iter()
                        .map(|cell| cell.symbol())
                        .collect();
                    assert!(text.contains("1  Automatic: GC — Goal — HULKs"));
                    assert!(text.contains("2  Automatic: GC — Goal — Invisibles"));
                    app.controller = None;
                }
                1 => {
                    assert!(text.contains("g  Start official game (manual)"));
                    assert!(text.contains("1  Unavailable: finished"));
                    assert!(text.contains("t  Unavailable: finished"));
                    app.controller = Some(crate::game_controller::Receiver::for_test());
                    terminal.draw(|frame| draw(frame, &mut app)).unwrap();
                    let text: String = terminal
                        .backend()
                        .buffer()
                        .content
                        .iter()
                        .map(|cell| cell.symbol())
                        .collect();
                    assert!(text.contains("g  Waiting for GC opponent change"));
                    assert!(!text.contains("g  Start official game (manual)"));
                    app.controller = None;
                }
                _ => {
                    assert!(text.contains("g  Game start"));
                    assert!(text.contains("h  Half-time"));
                    assert!(text.contains("t  Unavailable: new session needed"));
                    assert!(text.contains("f  Official game end"));
                    assert!(text.contains("2  Goal — HTWK"));
                }
            }
        }
    }

    #[test]
    fn fixed_golden_goal_has_its_own_score_and_a_finished_status() {
        let metadata = crate::model::Metadata {
            event: "Hobbymesse".into(),
            home: "Invisibles".into(),
            away: "HULKs".into(),
            date: captured_time().date_naive(),
            label: "golden_goal".into(),
            robots: std::array::from_fn(|_| String::new()),
        };
        let mut app = App::new(None).unwrap();
        app.session = Some(crate::model::Session::new_match(
            metadata,
            MatchType::GoldenGoal,
        ));
        app.mode = Mode::Idle;
        for finished in [false, true] {
            if finished {
                let session = app.session.as_mut().unwrap();
                session.record(EventKind::GoldenGoalStart, captured_time());
                session.record(EventKind::GoalAway, captured_time());
                session.record(EventKind::GameEnd, captured_time());
            }
            for (width, height) in [(60, 16), (140, 40)] {
                let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                terminal.draw(|frame| draw(frame, &mut app)).unwrap();
                let text: String = terminal
                    .backend()
                    .buffer()
                    .content
                    .iter()
                    .map(|cell| cell.symbol())
                    .collect();
                assert!(text.contains("GOLDEN GOAL"));
                assert!(!text.contains("GOLDEN GOAL TEST"));
                assert!(!text.contains("match:"));
                assert!(!text.contains("OFFICIAL"));
                if finished {
                    assert!(text.contains("GOLDEN GOAL • FINISHED"));
                    assert!(text.contains("Invisibles 0:1 HULKs"));
                    assert!(!text.contains("manual goals enabled"));
                } else {
                    assert!(text.contains("Invisibles 0:0 HULKs"));
                    assert!(!text.contains("FINISHED"));
                }
            }
        }
    }

    #[test]
    fn fixed_match_action_labels_show_type_and_finished_restrictions() {
        for match_type in [MatchType::Official, MatchType::GoldenGoal] {
            for finished in [false, true] {
                let metadata = crate::model::Metadata {
                    event: "Hobbymesse".into(),
                    home: "HULKs".into(),
                    away: if match_type == MatchType::Official {
                        "HTWK"
                    } else {
                        "Invisibles"
                    }
                    .into(),
                    date: captured_time().date_naive(),
                    label: "game".into(),
                    robots: std::array::from_fn(|_| String::new()),
                };
                let mut app = App::new(None).unwrap();
                let mut session = crate::model::Session::new_match(metadata, match_type);
                if finished {
                    session.record(EventKind::GameEnd, captured_time());
                    session.delete(0);
                    session.record(
                        if match_type == MatchType::Official {
                            EventKind::SecondHalfStart
                        } else {
                            EventKind::GoldenGoalStart
                        },
                        captured_time(),
                    );
                    session.restore(0);
                }
                app.session = Some(session);
                let mut terminal = Terminal::new(TestBackend::new(60, 16)).unwrap();
                for (selected, (shortcut, action)) in ACTIONS.iter().enumerate() {
                    app.mode = Mode::Actions {
                        selected,
                        timestamp: captured_time(),
                        gc_seconds_remaining: None,
                        message_budget: None,
                    };
                    terminal.draw(|frame| draw(frame, &mut app)).unwrap();
                    let text: String = terminal
                        .backend()
                        .buffer()
                        .content
                        .iter()
                        .map(|cell| cell.symbol())
                        .collect();
                    assert!(text.contains(if match_type == MatchType::Official {
                        "OFFICIAL"
                    } else {
                        "GOLDEN GOAL"
                    }));
                    if finished && match_type == MatchType::Official {
                        assert!(text.contains("OFFICIAL • FULL TIME"));
                        assert!(!text.contains("OFFICIAL • SECOND HALF"));
                    }
                    let label = match action {
                        Action::Mark(kind)
                            if finished && !matches!(kind, EventKind::Note | EventKind::Custom) =>
                        {
                            "Unavailable: finished"
                        }
                        Action::Mark(EventKind::HalfTime | EventKind::SecondHalfStart)
                            if match_type == MatchType::GoldenGoal =>
                        {
                            "Unavailable: Golden Goal"
                        }
                        Action::Mark(EventKind::GoldenGoalStart)
                            if match_type == MatchType::Official =>
                        {
                            "Unavailable: new session needed"
                        }
                        Action::Mark(EventKind::GameStart | EventKind::GoldenGoalStart)
                            if match_type == MatchType::GoldenGoal =>
                        {
                            "Golden Goal start"
                        }
                        Action::Mark(EventKind::GoalHome) => "Goal — HULKs",
                        Action::Mark(EventKind::GoalAway) => "Goal —",
                        _ => action.label(),
                    };
                    let expected: String =
                        format!("▶ {shortcut}  {label}").chars().take(38).collect();
                    assert!(text.contains(&expected), "Missing {expected}");
                }
            }
        }
    }

    #[test]
    fn action_menu_shows_captured_wall_and_gc_times_at_minimum_size() {
        let mut app = App::new(None).unwrap();
        let mut terminal = Terminal::new(TestBackend::new(60, 16)).unwrap();
        for (clock, expected) in [
            (Some(545), "GC 09:05"),
            (Some(-1), "GC -00:01"),
            (None, "GC unavailable"),
        ] {
            app.mode = Mode::Actions {
                selected: 0,
                timestamp: captured_time(),
                gc_seconds_remaining: clock,
                message_budget: None,
            };
            terminal.draw(|frame| draw(frame, &mut app)).unwrap();
            let text: String = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect();
            assert!(text.contains("14:32:08.123 +02:00"));
            assert!(text.contains(expected));
            assert!(text.contains("captured on Tab"));
        }
    }

    #[test]
    fn removed_events_remain_visible_but_are_marked_undone() {
        let mut app = App::new(None).unwrap();
        let metadata = crate::model::Metadata {
            event: "Hobbymesse".into(),
            home: "HULKs".into(),
            away: "HTWK".into(),
            date: captured_time().date_naive(),
            label: "game".into(),
            robots: std::array::from_fn(|_| String::new()),
        };
        let mut session = crate::model::Session::new(metadata);
        session.record(EventKind::GoalHome, captured_time());
        session.delete(0);
        app.session = Some(session);
        app.mode = Mode::Idle;
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("[undone] Goal home"));
        assert!(text.contains("HULKs 0:0 HTWK"));
        assert!(!app.preview().contains("Goal HULKs"));
    }

    #[test]
    fn each_operating_mode_has_a_distinct_label_and_color() {
        let mut app = App::new(None).unwrap();
        app.mode = Mode::Idle;
        assert_eq!(app.mode_style().1, Color::LightCyan);
        app.mode = Mode::Writing;
        assert_eq!(app.mode_style().1, Color::LightGreen);
        app.mode = Mode::Actions {
            selected: 0,
            timestamp: captured_time(),
            gc_seconds_remaining: None,
            message_budget: None,
        };
        assert_eq!(app.mode_style().1, Color::LightMagenta);
        app.mode = Mode::Delete(0);
        assert_eq!(app.mode_style().1, Color::LightRed);
        app.save_error = Some("disk full".into());
        assert!(app.mode_style().0.starts_with("SAVE FAILED"));
    }
}
