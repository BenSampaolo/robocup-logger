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
