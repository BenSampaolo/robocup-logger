use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{
        Block, BorderType, Borders, Clear, List, ListItem, ListState, Padding, Paragraph, Wrap,
    },
};

use crate::{
    app::{ACTIONS, Action, App, Form, FormKind, Mode},
    game_controller::Snapshot,
    model::{EventKind, MatchType, Session},
    navigation::Command,
    presentation::{ACCENT, ERROR, FOCUS, MUTED, SUCCESS, popup, solid_panel},
    teams::{display_name, team_number},
};

pub fn draw(frame: &mut Frame, app: &mut App) {
    let now = std::time::Instant::now();
    if let Some(session) = &app.session {
        app.arcade.observe(session, now);
    }
    let area = frame.area();
    app.controls_visible = area.width >= 60 && area.height >= 16;
    frame.render_widget(
        Block::default().style(Style::default().bg(Color::Black).fg(Color::Gray)),
        area,
    );
    if !app.controls_visible {
        frame.render_widget(Paragraph::new("Please resize to at least 60 columns × 16 rows.\nYour notes are preserved; resize to see the controls.").wrap(Wrap { trim: false }), area);
        return;
    }
    let rows = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(8),
        Constraint::Length(1),
    ])
    .split(area);
    let columns = Layout::horizontal([Constraint::Ratio(1, 3); 3]).split(rows[1]);
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
    draw_preview(frame, app, columns[2]);
    match &mut app.mode {
        Mode::Form(form) => draw_form(frame, form, columns[0].union(columns[1])),
        _ => {
            draw_log(frame, app, columns[0]);
            let middle =
                Layout::vertical([Constraint::Length(4), Constraint::Min(0)]).split(columns[1]);
            let gc_clock = app
                .controller
                .as_ref()
                .and_then(|receiver| receiver.live_clock(now));
            app.arcade.draw(frame, middle[0], now, gc_clock);
            crate::field::draw(frame, middle[1], &app.field_dimensions, &app.telemetry);
        }
    }
    match &mut app.mode {
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
                .map(|action| {
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
                                "Waiting for GC official restart / opponent change".into()
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
                        Action::Controller if app.controller.is_some() => "Pause GC logging".into(),
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
                    ListItem::new(label).style(Style::default().fg(if disabled {
                        MUTED
                    } else if *action == Action::Delete {
                        ERROR
                    } else {
                        FOCUS
                    }))
                })
                .collect();
            let inner = popup(
                frame,
                rows[1],
                &format!("Actions @ {}", timestamp.format("%H:%M:%S%.3f %:z")),
                ACCENT,
                76,
                ACTIONS.len() as u16 + 3,
            );
            let sections =
                Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(inner);
            let list = List::new(items)
                .highlight_style(
                    Style::default()
                        .bg(ACCENT)
                        .fg(Color::Black)
                        .add_modifier(Modifier::BOLD),
                )
                .highlight_symbol("▶ ");
            frame.render_stateful_widget(
                list,
                sections[0],
                &mut ListState::default().with_selected(Some(*selected)),
            );
            frame.render_widget(
                Paragraph::new(format!(
                    "GC {} — captured on opening",
                    gc_seconds_remaining
                        .map(crate::game_controller::format_clock)
                        .unwrap_or_else(|| "unavailable".into())
                ))
                .style(Style::default().fg(FOCUS)),
                sections[1],
            );
        }
        Mode::ReconnectController { selected } => {
            let inner = popup(
                frame,
                rows[1],
                "Reconnect/switch GC — same match?",
                FOCUS,
                66,
                10,
            );
            let sections =
                Layout::vertical([Constraint::Min(1), Constraint::Length(2)]).split(inner);
            frame.render_widget(
                Paragraph::new("Keep history and raw capture; reacquire the current opponent. First matching GC wins: this may reconnect to the same controller.")
                    .style(Style::default().fg(FOCUS)).wrap(Wrap { trim: false }),
                sections[0],
            );
            frame.render_stateful_widget(
                List::new(["Cancel — keep current GC", "Reconnect/switch GC"])
                    .highlight_symbol("▶ ")
                    .highlight_style(Style::default().fg(Color::Black).bg(ACCENT)),
                sections[1],
                &mut ListState::default().with_selected(Some(*selected)),
            );
        }
        Mode::Delete { index, selected } => {
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
            let inner = popup(frame, rows[1], "Remove event? (reversible)", FOCUS, 56, 9);
            let sections =
                Layout::vertical([Constraint::Min(1), Constraint::Length(2)]).split(inner);
            frame.render_widget(
                Paragraph::new(description)
                    .style(Style::default().fg(FOCUS))
                    .wrap(Wrap { trim: false }),
                sections[0],
            );
            frame.render_stateful_widget(
                List::new([
                    ListItem::new("Cancel — keep event").style(Style::default().fg(SUCCESS)),
                    ListItem::new("Remove event (mark undone)").style(Style::default().fg(ERROR)),
                ])
                .highlight_symbol("▶ ")
                .highlight_style(Style::default().fg(Color::Black).bg(ACCENT)),
                sections[1],
                &mut ListState::default().with_selected(Some(*selected)),
            );
        }
        Mode::Form(form) if form.discard_selected.is_some() => {
            let inner = popup(frame, rows[1], "Discard form changes?", FOCUS, 56, 9);
            let sections =
                Layout::vertical([Constraint::Min(1), Constraint::Length(2)]).split(inner);
            frame.render_widget(
                Paragraph::new(
                    "Unsaved form changes will be lost. Previously saved data is unchanged.",
                )
                .style(Style::default().fg(FOCUS))
                .wrap(Wrap { trim: false }),
                sections[0],
            );
            frame.render_stateful_widget(
                List::new([
                    ListItem::new("Cancel — keep editing").style(Style::default().fg(SUCCESS)),
                    ListItem::new("Discard changes").style(Style::default().fg(ERROR)),
                ])
                .highlight_symbol("▶ ")
                .highlight_style(Style::default().fg(Color::Black).bg(ACCENT)),
                sections[1],
                &mut ListState::default().with_selected(form.discard_selected),
            );
        }
        Mode::Form(form) if form.detected_teams.is_some() => {
            let detected = form.detected_teams.as_ref().unwrap();
            let inner = popup(
                frame,
                rows[1],
                "Detected GC teams — Official match",
                ACCENT,
                70,
                10,
            );
            let sections =
                Layout::vertical([Constraint::Min(2), Constraint::Length(3)]).split(inner);
            frame.render_widget(
                Paragraph::new("Set both teams and switch to Official match?")
                    .style(Style::default().fg(FOCUS))
                    .wrap(Wrap { trim: false }),
                sections[0],
            );
            frame.render_stateful_widget(
                List::new([
                    ListItem::new("Cancel — keep setup unchanged"),
                    ListItem::new(format!("Official match — track {}", detected.teams[0])),
                    ListItem::new(format!("Official match — track {}", detected.teams[1])),
                ])
                .highlight_symbol("▶ ")
                .highlight_style(Style::default().fg(Color::Black).bg(ACCENT)),
                sections[1],
                &mut ListState::default().with_selected(Some(detected.selected)),
            );
        }
        Mode::Form(form) if form.team_selection.is_some() => {
            let inner = popup(
                frame,
                rows[1],
                form.fields[form.selected].label,
                ACCENT,
                56,
                14,
            );
            let sections =
                Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).split(inner);
            frame.render_widget(
                Paragraph::new(format!(
                    "{}: {}",
                    if form.team_searching {
                        "Search"
                    } else {
                        "Filter"
                    },
                    if form.team_filter.is_empty() {
                        "(all teams)"
                    } else {
                        &form.team_filter
                    },
                ))
                .style(Style::default().fg(FOCUS)),
                sections[0],
            );
            let visible = form.visible_teams();
            if visible.is_empty() {
                frame.render_widget(Paragraph::new("No matching teams"), sections[1]);
            } else {
                let selected = visible
                    .iter()
                    .position(|index| Some(*index) == form.team_selection);
                frame.render_stateful_widget(
                    List::new(visible.iter().map(|index| {
                        let (number, name) = form.team_choices()[*index];
                        ListItem::new(format!("{name} (#{number})"))
                    }))
                    .highlight_symbol("▶ ")
                    .highlight_style(Style::default().fg(Color::Black).bg(ACCENT)),
                    sections[1],
                    &mut ListState::default().with_selected(selected),
                );
            }
        }
        _ => {}
    }
    draw_footer(frame, app, rows[2]);
    if app.help_visible {
        let help_area = Rect {
            x: area.x + 1,
            y: area.y + 1,
            width: area.width.saturating_sub(2),
            height: area.height.saturating_sub(2).min(16),
        };
        let hint = |command| app.keys.hint(command);
        let help = format!(
            "{}/{}: previous/next\n{}/{}: previous/next or change option\n{}: select/edit\n{}: back/cancel; idle Esc: actions\n{}: help\nField: Enter/Esc finishes editing.\nLive note: Enter starts; Enter/Esc saves.\nCtrl+C: save/quit.",
            hint(Command::Up),
            hint(Command::Down),
            hint(Command::Left),
            hint(Command::Right),
            hint(Command::Confirm),
            hint(Command::Back),
            hint(Command::Help),
        );
        frame.render_widget(Clear, help_area);
        let inner = solid_panel(
            frame,
            help_area,
            panel(
                "Help — confirm/back closes",
                color,
                u16::from(help_area.height >= 16),
            ),
            color,
        );
        frame.render_widget(Paragraph::new(help).wrap(Wrap { trim: false }), inner);
    }
}

fn panel(title: impl Into<Line<'static>>, color: Color, vertical_padding: u16) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Thick)
        .padding(Padding::new(2, 2, vertical_padding, vertical_padding))
        .border_style(Style::default().fg(color))
        .title(title)
}

fn opponent_warning(session: &Session, packet: &Snapshot) -> Option<(String, Color)> {
    let pending_official = session.match_type == Some(MatchType::Combined)
        && session.in_golden_goal()
        && session.is_finished()
        && session
            .golden_goal_opponent
            .is_some_and(|opponent| packet.starts_official_after(opponent));
    let (home, away) = if pending_official {
        (
            session.metadata.home.as_str(),
            session.metadata.away.as_str(),
        )
    } else {
        session.current_teams()
    };
    let configured = if session.metadata.tracked_are_home() {
        away
    } else {
        home
    };
    let received = packet.opponent_number;
    let received_name = display_name(received);
    match team_number(configured) {
        Some(expected) if expected == received => None,
        Some(_) => Some((
            format!("OPPONENT MISMATCH: set {configured}; GC {received_name}"),
            FOCUS,
        )),
        None => Some((
            format!("OPPONENT UNVERIFIED: {configured}; GC {received_name}"),
            FOCUS,
        )),
    }
}

fn controller_status(app: &App) -> Line<'static> {
    if let Some(error) = &app.controller_error {
        return Line::styled(format!(" GC ERROR: {error}"), Style::default().fg(ERROR));
    }
    let Some(controller) = &app.controller else {
        let hint = if app.session.as_ref().is_some_and(|session| {
            session.match_type == Some(MatchType::Combined)
                && session.in_golden_goal()
                && session.is_finished()
        }) {
            "waiting for official restart • Actions: start manually"
        } else if app
            .session
            .as_ref()
            .is_some_and(|session| session.match_type.is_some() && session.is_finished())
        {
            "match finished • notes/corrections available"
        } else {
            "manual goals enabled • Actions: Resume GC logging"
        };
        let (status, reachable) = app.discovery.status();
        return Line::from(vec![
            Span::styled(" GC OFF / NOT IMPORTING • ", Style::default().fg(MUTED)),
            Span::styled(
                status,
                Style::default().fg(if reachable { SUCCESS } else { MUTED }),
            ),
            Span::styled(format!(" • {hint}"), Style::default().fg(MUTED)),
        ]);
    };
    let (transport, mut color) = if controller.latest.is_none() {
        ("waiting for feed", FOCUS)
    } else if controller.is_stale() {
        ("stale feed", FOCUS)
    } else {
        ("live feed", SUCCESS)
    };
    let label = if app.session.as_ref().is_some_and(|session| {
        session.match_type == Some(MatchType::Combined)
            && session.current_game() == MatchType::GoldenGoal
            && session.is_finished()
    }) {
        color = FOCUS;
        "WAITING OFFICIAL"
    } else if app
        .session
        .as_ref()
        .is_some_and(|session| session.match_type.is_some() && session.is_finished())
    {
        color = MUTED;
        "FINISHED"
    } else if controller.latest.is_none() || controller.is_stale() {
        "NOT IMPORTING"
    } else if !app.pending_controller.is_empty() {
        "IMPORT QUEUED"
    } else {
        "IMPORTING"
    };
    let detail = match (&controller.latest, &app.session) {
        (Some(packet), Some(session)) => {
            let (home, away) = if session.metadata.tracked_are_home() {
                (packet.tracked_score, packet.opponent_score)
            } else {
                (packet.opponent_score, packet.tracked_score)
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
                controller
                    .live_clock(std::time::Instant::now())
                    .unwrap_or_else(|| packet.clock()),
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
                " UDP 3838 • goals automatic; Actions: Pause GC logging".into()
            }
        }
    };
    let detail = controller
        .notice
        .as_ref()
        .map_or(detail, |notice| format!(" {notice}"));
    let detail = if controller.latest.is_none() {
        let (status, _) = controller.discovery_status();
        format!(" {status} • waiting for selected match •{detail}")
    } else {
        detail
    };
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
        Span::styled(format!("{transport} •{detail}"), Style::default().fg(color)),
    ])
}

fn match_status(app: &App) -> Line<'static> {
    if let Mode::Form(form) = &app.mode
        && !matches!(form.kind, FormKind::Entry(_))
    {
        let tracked = team_number(form.fields[1].value());
        let opponent = team_number(form.fields[2].value());
        let match_type = match form.match_type {
            Some(MatchType::Combined) => "COMBINED • ",
            Some(MatchType::Official) => "OFFICIAL • ",
            Some(MatchType::GoldenGoal) => "GOLDEN GOAL • ",
            None => "",
        };
        return if tracked.is_none() || opponent.is_none() || tracked == opponent {
            Line::styled(
                format!(" {match_type}Choose two different known teams"),
                Style::default().fg(ERROR),
            )
        } else {
            Line::styled(
                format!(
                    " {match_type}Tracked: {} • Opponent: {}",
                    form.fields[1].value(),
                    form.fields[2].value(),
                ),
                Style::default().fg(SUCCESS),
            )
        };
    }
    let Some(session) = &app.session else {
        return Line::from(" New match");
    };
    let combined = session.match_type == Some(MatchType::Combined);
    let test = session.in_golden_goal();
    let phase = if test && session.match_type.is_some() {
        if session.is_finished() {
            if combined {
                ("GOLDEN GOAL • waiting for official restart", FOCUS)
            } else {
                ("GOLDEN GOAL • FINISHED", Color::Gray)
            }
        } else {
            ("GOLDEN GOAL", ACCENT)
        }
    } else if session.match_type.is_some() && session.is_finished() {
        ("FULL TIME", Color::Gray)
    } else if test {
        ("GOLDEN GOAL TEST", ACCENT)
    } else {
        session
            .entries
            .iter()
            .rev()
            .filter(|entry| {
                !entry.undone && (!combined || entry.game == Some(session.current_game()))
            })
            .find_map(|entry| match entry.kind {
                EventKind::GameStart => Some(("FIRST HALF", SUCCESS)),
                EventKind::HalfTime => Some(("HALFTIME", FOCUS)),
                EventKind::SecondHalfStart => Some(("SECOND HALF", SUCCESS)),
                EventKind::GameEnd => Some(("FULL TIME", Color::Gray)),
                _ => None,
            })
            .unwrap_or(("PRE-GAME", ACCENT))
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
        let (home_team, away_team) = if session.metadata.tracked_are_home() {
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
            Style::default().fg(FOCUS).add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!(
            " • {}: {} • {detail}",
            session.metadata.tracked_team_name(),
            if session.metadata.tracked_are_home() {
                "home"
            } else {
                "away"
            }
        )),
    ])
}

fn draw_form(frame: &mut Frame, form: &mut Form, area: Rect) {
    let title = match form.kind {
        FormKind::Setup => "Match setup — select Create",
        FormKind::Metadata => "Match details — select Save",
        FormKind::Entry(_) => "Correct event — select Save",
    };
    let inner = solid_panel(
        frame,
        area,
        panel(title, ACCENT, u16::from(area.height >= 14)),
        ACCENT,
    );
    let setup = matches!(form.kind, FormKind::Setup);
    let sections = Layout::vertical([
        Constraint::Min(3),
        Constraint::Length(if setup { 2 } else { 1 }),
    ])
    .split(inner);
    let fields_area = sections[0];
    let field_height = if fields_area.height >= 15 { 5 } else { 3 };
    let count = (fields_area.height as usize / field_height as usize)
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
            y: fields_area.y + visible as u16 * field_height,
            width: fields_area.width,
            height: field_height.min(
                fields_area
                    .height
                    .saturating_sub(visible as u16 * field_height),
            ),
        };
        let color = if selected { ACCENT } else { MUTED };
        let inner = solid_panel(
            frame,
            field_area,
            panel(
                format!(
                    "{} {}{}",
                    if selected { "▶" } else { " " },
                    field.label,
                    if selected && form.editing {
                        " (editing)"
                    } else {
                        ""
                    }
                ),
                color,
                u16::from(field_height == 5),
            ),
            color,
        );
        field.editor.remove_block();
        field
            .editor
            .set_style(Style::default().fg(if selected { FOCUS } else { Color::Gray }));
        field.editor.set_cursor_line_style(Style::default());
        field.editor.set_cursor_style(if selected && form.editing {
            Style::default().bg(ACCENT).fg(Color::Black)
        } else {
            Style::default()
        });
        frame.render_widget(&field.editor, inner);
    }
    let save = if matches!(form.kind, FormKind::Setup) {
        "Create match"
    } else {
        "Save"
    };
    let buttons: Vec<Span> = [save, "Cancel"]
        .into_iter()
        .enumerate()
        .map(|(index, label)| {
            let selected = form.selected == form.fields.len() + index;
            Span::styled(
                format!("{}[ {label} ] ", if selected { "▶" } else { " " }),
                if selected {
                    Style::default()
                        .bg(ACCENT)
                        .fg(Color::Black)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(if index == 0 { SUCCESS } else { ACCENT })
                },
            )
        })
        .collect();
    let mut buttons = vec![Line::from(buttons)];
    if setup {
        let selected = form.selected == form.fields.len() + 2;
        buttons.insert(
            0,
            Line::styled(
                format!(
                    "{}[ Use detected GC teams ]",
                    if selected { "▶" } else { " " }
                ),
                if selected {
                    Style::default()
                        .bg(ACCENT)
                        .fg(Color::Black)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(ACCENT)
                },
            ),
        );
    }
    frame.render_widget(Paragraph::new(buttons), sections[1]);
}

fn draw_log(frame: &mut Frame, app: &mut App, area: Rect) {
    let note_height = if area.height >= 14 { 6 } else { 4 };
    let sections =
        Layout::vertical([Constraint::Min(3), Constraint::Length(note_height)]).split(area);
    let items: Vec<ListItem> = app
        .session
        .as_ref()
        .map(|session| {
            session
                .entries
                .iter()
                .enumerate()
                .map(|(index, entry)| {
                    let color = if session.active_note == Some(index) {
                        FOCUS
                    } else {
                        Color::Gray
                    };
                    let mut lines = vec![Line::from(vec![
                        Span::styled(
                            format!("{} ", entry.timestamp.format("%H:%M:%S%.3f")),
                            Style::default().fg(MUTED),
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
                            Style::default().fg(FOCUS),
                        ),
                    ])];
                    if !entry.text.is_empty() {
                        lines.push(Line::from(format!("  {}", entry.display_text())));
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
    let inner = solid_panel(
        frame,
        sections[0],
        panel(
            "Events — idle Esc: actions",
            MUTED,
            u16::from(sections[0].height >= 7),
        ),
        MUTED,
    );
    let list = List::new(items)
        .style(Style::default().fg(MUTED))
        .highlight_style(Style::default().bg(ACCENT).fg(Color::Black))
        .highlight_symbol("▶ ");
    frame.render_stateful_widget(list, inner, &mut app.history);
    if app
        .session
        .as_ref()
        .is_some_and(|session| session.active_note.is_some())
    {
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
        let inner = solid_panel(
            frame,
            sections[1],
            panel(
                format!("Note @ {timestamp} — Enter/Esc saves"),
                ACCENT,
                u16::from(note_height == 6),
            ),
            ACCENT,
        );
        app.note_editor.remove_block();
        app.note_editor.set_style(Style::default().fg(FOCUS));
        app.note_editor
            .set_cursor_style(Style::default().fg(Color::Black).bg(ACCENT));
        app.note_editor.set_cursor_line_style(Style::default());
        frame.render_widget(&app.note_editor, inner);
    } else {
        let inner = solid_panel(
            frame,
            sections[1],
            panel("Ready", ACCENT, u16::from(note_height == 6)),
            ACCENT,
        );
        frame.render_widget(
            Paragraph::new("Enter: new note").style(Style::default().fg(FOCUS)),
            inner,
        );
    }
}

fn draw_preview(frame: &mut Frame, app: &mut App, area: Rect) {
    let text = app.preview();
    let lines: Vec<Line> = text
        .lines()
        .map(|line| Line::styled(line.to_owned(), Style::default().fg(MUTED)))
        .collect();
    let title = if matches!(app.mode, Mode::Form(_)) {
        "Markdown preview (form not applied)"
    } else {
        "Markdown preview"
    };
    let inner = solid_panel(
        frame,
        area,
        panel(title, MUTED, u16::from(area.height >= 14)),
        MUTED,
    );
    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
    let max_scroll = paragraph
        .line_count(inner.width)
        .saturating_sub(inner.height as usize)
        .min(u16::MAX as usize) as u16;
    app.preview_max = max_scroll;
    let scroll = app.preview_scroll.unwrap_or(max_scroll).min(max_scroll);
    frame.render_widget(paragraph.scroll((scroll, 0)), inner);
}

fn draw_footer(frame: &mut Frame, app: &App, area: Rect) {
    let hint = |command| app.keys.hint(command);
    let menu_help = format!(
        "{}: choose  {}: cancel  {}: help",
        hint(Command::Confirm),
        hint(Command::Back),
        hint(Command::Help)
    );
    let selection_help = format!(
        "{}/{}: select field or Save/Cancel",
        hint(Command::Up),
        hint(Command::Down)
    );
    let (help, extra) = match &app.mode {
        Mode::Form(form) if form.discard_selected.is_some() => (menu_help, "".into()),
        Mode::Form(form) if form.detected_teams.is_some() => (
            menu_help,
            "Choose tracked team AND Official match type; only the draft changes.".into(),
        ),
        Mode::Form(form) if form.team_searching => (
            "Search teams • type to filter • Enter: finish search".into(),
            "Arrows: select • Esc: clear".into(),
        ),
        Mode::Form(form) if form.team_selection.is_some() => (
            "Enter/Space: choose • Esc: clear filter/cancel".into(),
            format!(
                "{}/{} or arrows: select • /: search (unless bound)",
                hint(Command::Up),
                hint(Command::Down)
            ),
        ),
        Mode::Form(form) if form.editing => ("Editing • Enter/Esc: done".into(), String::new()),
        Mode::Form(form) => (
            menu_help,
            if matches!(form.kind, FormKind::Setup) && form.selected == form.fields.len() + 2 {
                "Use fresh GC teams: choose tracked team and confirm Official type".into()
            } else if form.is_team_field() {
                "Enter/Space: open team dropdown; / inside dropdown: search".into()
            } else if form.is_choice_field() {
                format!(
                    "{}/{}: change options",
                    hint(Command::Left),
                    hint(Command::Right)
                )
            } else {
                selection_help
            },
        ),
        Mode::Actions { .. } => (
            menu_help,
            format!("{}/{}: select", hint(Command::Up), hint(Command::Down)),
        ),
        Mode::Delete { .. } | Mode::ReconnectController { .. } => (menu_help, "".into()),
        Mode::Writing => ("Enter/Esc: save note • Esc: actions".into(), String::new()),
        Mode::Idle => (
            format!(
                "Enter: mark note  Esc: actions  {}: help",
                hint(Command::Help)
            ),
            format!(
                "{}/{}: select history • Ctrl+C: save/quit",
                hint(Command::Up),
                hint(Command::Down)
            ),
        ),
    };
    let state = if !app.pending_controller.is_empty() {
        "GC QUEUED"
    } else if app.dirty {
        "UNSAVED"
    } else if app.store.is_some() {
        "SAVED"
    } else {
        "SETUP"
    };
    let (message, color) = if let Some(error) = &app.save_error {
        (error.as_str(), ERROR)
    } else {
        (app.status.as_str(), FOCUS)
    };
    let text = [state, message, &help, &extra]
        .into_iter()
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join(" · ")
        .replace('\n', " · ");
    frame.render_widget(Paragraph::new(text).style(Style::default().fg(color)), area);
}
