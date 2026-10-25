use std::{
    f64::consts::PI,
    io,
    time::{Duration, Instant},
};

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Style, Stylize},
    symbols::Marker,
    text::{Line, Span},
    widgets::{
        Block, Paragraph, Wrap,
        canvas::{Canvas, Circle, Context, Line as Segment, Points, Rectangle},
    },
};

use crate::{
    arcade::{self, Cue, Effect},
    field::FieldDimensions,
    model::{EventKind, MatchType, Session},
    presentation,
    replay_data::Timeline,
    storage::Store,
};

const BLUE: Color = Color::LightCyan;
const RED: Color = Color::LightRed;
const BACKGROUND: Color = Color::Rgb(18, 8, 28);
const OWN_COLORS: [Color; 5] = [
    Color::LightCyan,
    Color::LightGreen,
    Color::LightYellow,
    Color::LightMagenta,
    Color::LightRed,
];

struct Scoreboard {
    score: (usize, usize),
    game: MatchType,
}

impl Scoreboard {
    fn from_session(session: &Session) -> Self {
        Self {
            score: if session.in_golden_goal() {
                session.golden_goal_score()
            } else {
                session.score()
            },
            game: session.current_game(),
        }
    }
}

pub(crate) struct Replay {
    pub time: f64,
    pub paused: bool,
    timeline: Timeline,
    scoreboards: Vec<Scoreboard>,
    manual_speed: Option<f64>,
    balls: bool,
    show_opponents: bool,
    selected_robot: Option<(bool, u8)>,
    note_cursor: Option<usize>,
    note_scroll: u16,
    markers: Vec<f64>,
    controller_events: Vec<usize>,
    cue: Option<Cue>,
}

impl Replay {
    pub(crate) fn load(store: &Store, session: &Session) -> io::Result<Self> {
        if session.field_dimensions.is_none() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Confirm the match's field dimensions before replay",
            ));
        }
        let timeline = Timeline::load(store, session)?;
        let now = Instant::now();
        let mut markers: Vec<_> = timeline.notes.iter().map(|note| note.0).collect();
        let mut past = Session {
            match_type: session.match_type,
            golden_goal_opponent: session.golden_goal_opponent,
            official_opponent: session.official_opponent,
            ..Session::new(session.metadata.clone())
        };
        let mut scoreboards = vec![Scoreboard::from_session(&past)];
        for &(time, index) in &timeline.events {
            past.entries.push(session.entries[index].clone());
            scoreboards.push(Scoreboard::from_session(&past));
            if Cue::from_entry(&past, &session.entries[index], now).is_some() {
                markers.push(time);
            }
        }
        let mut controller_events = Vec::new();
        for (index, pair) in timeline.controllers.windows(2).enumerate() {
            let (previous, current) = (&pair[0], &pair[1]);
            if current.continuous
                && current.source == previous.source
                && current.snapshot.opponent_number == previous.snapshot.opponent_number
                && (!current.snapshot.same_game_state(&previous.snapshot)
                    || Cue::from_controller(&previous.snapshot, &current.snapshot, now).is_some())
            {
                markers.push(current.time);
                controller_events.push(index + 1);
            }
        }
        markers.sort_by(f64::total_cmp);
        markers.dedup();
        let position = session
            .replay_position
            .or_else(|| {
                session.postgame_position.and_then(|index| {
                    timeline
                        .notes
                        .iter()
                        .find(|note| note.1 == index)
                        .map(|note| note.0)
                })
            })
            .unwrap_or(0.0);
        let mut replay = Self {
            time: 0.0,
            paused: true,
            timeline,
            scoreboards,
            manual_speed: None,
            balls: false,
            show_opponents: true,
            selected_robot: None,
            note_cursor: None,
            note_scroll: 0,
            markers,
            controller_events,
            cue: None,
        };
        replay.seek(position);
        if let Some(index) = session.postgame_position {
            if replay.timeline.entry_time(index) == Some(replay.time) {
                replay.seek_entry(index);
            }
        }
        Ok(replay)
    }

    pub(crate) fn note_entries(&self) -> Vec<usize> {
        self.timeline.notes.iter().map(|note| note.1).collect()
    }

    pub(crate) fn current_entry(&self) -> Option<usize> {
        self.note_cursor.map(|cursor| self.timeline.notes[cursor].1)
    }

    pub(crate) fn seek_entry(&mut self, index: usize) {
        if let Some(cursor) = self.timeline.notes.iter().position(|note| note.1 == index) {
            self.seek(self.timeline.notes[cursor].0);
            self.note_cursor = Some(cursor);
        }
    }

    fn seek(&mut self, time: f64) {
        self.time = time.clamp(0.0, self.timeline.duration);
        self.paused = true;
        self.cue = None;
        self.note_scroll = 0;
        self.note_cursor = self
            .timeline
            .notes
            .iter()
            .position(|note| note.0 == self.time)
            .or_else(|| {
                self.timeline
                    .notes
                    .iter()
                    .rposition(|note| note.0 < self.time)
            });
    }

    fn speed(&self) -> f64 {
        self.manual_speed.unwrap_or_else(|| {
            if self
                .markers
                .iter()
                .any(|&time| self.time >= time - 5.0 && self.time < time + 3.0)
            {
                1.0
            } else {
                8.0
            }
        })
    }

    pub(crate) fn advance(&mut self, elapsed: Duration, now: Instant, session: &Session) {
        if self
            .cue
            .as_ref()
            .is_some_and(|cue| now.duration_since(cue.started) >= cue.duration())
        {
            self.cue = None;
        }
        if self.paused {
            return;
        }
        let previous = self.time;
        let mut target =
            (previous + elapsed.as_secs_f64().min(0.25) * self.speed()).min(self.timeline.duration);
        if self.manual_speed.is_none() {
            for boundary in self
                .markers
                .iter()
                .flat_map(|time| [time - 5.0, time + 3.0])
            {
                if boundary > previous {
                    target = target.min(boundary);
                }
            }
        }
        if let Some((cursor, &(time, _))) = self
            .timeline
            .notes
            .iter()
            .enumerate()
            .find(|(_, note)| note.0 > previous && note.0 <= target)
        {
            target = time;
            self.note_cursor = Some(cursor);
            self.note_scroll = 0;
            self.paused = true;
        }
        self.time = target;
        self.paused |= self.time >= self.timeline.duration;
        let mut latest = None;
        let start = self
            .controller_events
            .partition_point(|&index| self.timeline.controllers[index].time <= previous);
        for &index in &self.controller_events[start..] {
            let sample = &self.timeline.controllers[index];
            if sample.time > target {
                break;
            }
            if let Some(cue) = Cue::from_controller(
                &self.timeline.controllers[index - 1].snapshot,
                &sample.snapshot,
                now,
            ) {
                latest = Some((sample.time, cue));
            }
        }
        let start = self
            .timeline
            .events
            .partition_point(|event| event.0 <= previous);
        for &(time, index) in &self.timeline.events[start..] {
            if time > target {
                break;
            }
            if latest
                .as_ref()
                .is_none_or(|(latest_time, _)| time >= *latest_time)
            {
                let context = Session {
                    match_type: Some(self.scoreboard(time).game),
                    official_opponent: session.official_opponent,
                    golden_goal_opponent: session.golden_goal_opponent,
                    ..Session::new(session.metadata.clone())
                };
                if let Some(cue) = Cue::from_entry(&context, &session.entries[index], now) {
                    latest = Some((time, cue));
                }
            }
        }
        if let Some((_, cue)) = latest {
            self.cue = Some(cue);
        }
    }

    pub(crate) fn key(&mut self, key: KeyEvent, _session: &Session) -> bool {
        if key.kind == KeyEventKind::Release
            || key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER)
        {
            return false;
        }
        let selection = match key.code {
            KeyCode::Char(number @ '1'..='5') => Some((true, number as u8 - b'0')),
            KeyCode::Char(number @ '6'..='9') => Some((false, number as u8 - b'5')),
            KeyCode::Char('0') => Some((false, 5)),
            _ => None,
        };
        if let Some(robot) = selection {
            self.selected_robot = (self.selected_robot != Some(robot)).then_some(robot);
            if !robot.0 {
                self.show_opponents = true;
            }
            return true;
        }
        match key.code {
            KeyCode::Char(' ') => {
                let next = self.note_cursor.map_or(0, |cursor| cursor + 1);
                if self.paused
                    && self
                        .timeline
                        .notes
                        .get(next)
                        .is_some_and(|note| note.0 == self.time)
                {
                    self.note_cursor = Some(next);
                    self.note_scroll = 0;
                } else {
                    self.paused = !self.paused;
                }
            }
            KeyCode::Left => self.seek(self.time - 5.0),
            KeyCode::Right => self.seek(self.time + 5.0),
            KeyCode::PageUp => self.note_scroll = self.note_scroll.saturating_sub(3),
            KeyCode::PageDown => self.note_scroll = self.note_scroll.saturating_add(3),
            KeyCode::Home => self.seek(0.0),
            KeyCode::End => self.seek(self.timeline.duration),
            KeyCode::Up => {
                let cursor = match self.note_cursor {
                    Some(cursor) if self.timeline.notes[cursor].0 == self.time => {
                        cursor.checked_sub(1)
                    }
                    cursor => cursor,
                };
                if let Some(cursor) = cursor {
                    self.seek_entry(self.timeline.notes[cursor].1);
                } else {
                    self.seek(0.0);
                }
            }
            KeyCode::Down => {
                let next = self.note_cursor.map_or(0, |cursor| cursor + 1);
                if let Some(&(_, index)) = self.timeline.notes.get(next) {
                    self.seek_entry(index);
                } else {
                    self.seek(self.timeline.duration);
                }
            }
            KeyCode::Char('+' | '=') => self.manual_speed = Some((self.speed() * 2.0).min(16.0)),
            KeyCode::Char('-') => self.manual_speed = Some((self.speed() / 2.0).max(0.5)),
            KeyCode::Char('r' | 'R') => self.manual_speed = None,
            KeyCode::Char('b' | 'B') => self.balls = !self.balls,
            KeyCode::Char('h' | 'H') => {
                self.show_opponents = !self.show_opponents;
                if !self.show_opponents && self.selected_robot.is_some_and(|robot| !robot.0) {
                    self.selected_robot = None;
                }
            }
            _ => return false,
        }
        true
    }

    fn scoreboard(&self, time: f64) -> &Scoreboard {
        &self.scoreboards[self
            .timeline
            .events
            .partition_point(|event| event.0 <= time)]
    }

    fn first_half(&self, session: &Session) -> bool {
        let controller = self.timeline.controller(self.time);
        let boundary = self.timeline.events.iter().rev().find(|&&(time, index)| {
            time <= self.time
                && matches!(
                    session.entries[index].kind,
                    EventKind::GameStart | EventKind::GoldenGoalStart | EventKind::SecondHalfStart
                )
        });
        if let Some(&(time, index)) = boundary {
            if controller.is_none_or(|sample| time >= sample.time) {
                return session.entries[index].kind != EventKind::SecondHalfStart;
            }
        }
        controller.is_none_or(|sample| sample.snapshot.first_half)
    }

    fn gc_clock(&self) -> (String, String) {
        let Some(sample) = self.timeline.controller(self.time) else {
            return ("--:--.--".into(), "GC UNAVAILABLE".into());
        };
        let age = self.time - sample.time;
        if age > 5.0 {
            return ("--:--.--".into(), "GC UNAVAILABLE".into());
        }
        let snapshot = &sample.snapshot;
        let running =
            snapshot.state == 3 && !snapshot.stopped && matches!(snapshot.game_phase, 0 | 2);
        let seconds = f64::from(snapshot.seconds_remaining) - if running { age } else { 0.0 };
        let hundredths = (seconds.abs() * 100.0).round() as u64;
        let title = format!(
            "{}{:02}:{:02}.{:02}",
            if seconds < 0.0 { "-" } else { "" },
            hundredths / 6000,
            hundredths / 100 % 60,
            hundredths % 100
        );
        let state = if snapshot.stopped {
            "Stopped"
        } else {
            match snapshot.state {
                0 => "Initial",
                1 => "Ready",
                2 => "Set",
                3 => "Playing",
                4 => "Finished",
                _ => "Unknown",
            }
        };
        let half = if snapshot.first_half {
            "1st half"
        } else {
            "2nd half"
        };
        let phase = match snapshot.game_phase {
            1 => "Shootout".to_owned(),
            2 => format!("Extra time · {half}"),
            3 => "Timeout".to_owned(),
            _ if self.scoreboard(self.time).game == MatchType::GoldenGoal => {
                "Golden Goal".to_owned()
            }
            _ => half.to_owned(),
        };
        (title, format!("GC · {phase} · {state}"))
    }

    pub(crate) fn draw(&mut self, frame: &mut Frame, area: Rect, session: &Session, now: Instant) {
        frame.render_widget(
            Block::default().style(Style::default().fg(Color::White).bg(Color::Black)),
            area,
        );
        if area.width < 60 || area.height < 22 {
            frame.render_widget(Paragraph::new("Enlarge replay to 60×22 or larger"), area);
            return;
        }
        let [header, pitch, note, progress, controls] = Layout::vertical([
            Constraint::Length(5),
            Constraint::Min(8),
            Constraint::Length(5),
            Constraint::Length(2),
            Constraint::Length(1),
        ])
        .areas(area);
        let [cards, info] =
            Layout::vertical([Constraint::Length(4), Constraint::Length(1)]).areas(header);
        let [timer, score, banner] = Layout::horizontal([
            Constraint::Percentage(35),
            Constraint::Percentage(25),
            Constraint::Percentage(40),
        ])
        .areas(cards);
        let scoreboard = self.scoreboard(self.time);
        let (home_score, away_score) = scoreboard.score;
        let (clock, clock_detail) = self.gc_clock();
        for (area, title, detail, color) in [
            (timer, clock, clock_detail, BLUE),
            (
                score,
                format!("{home_score}:{away_score}"),
                String::new(),
                Color::White,
            ),
        ] {
            arcade::draw(
                frame,
                area,
                Some(&Cue {
                    effect: Effect::Clock,
                    title,
                    detail,
                    color,
                    started: now,
                }),
                now,
                None,
            );
        }
        let accepted_opponent = self
            .timeline
            .controller(self.time)
            .map(|sample| sample.snapshot.opponent_number)
            .or(session.official_opponent)
            .map(crate::teams::display_name);
        let (home, away) = if scoreboard.game == MatchType::GoldenGoal {
            session.metadata.golden_goal_teams()
        } else if let Some(opponent) = accepted_opponent.as_deref() {
            if session.metadata.tracked_are_home() {
                (session.metadata.tracked_team_name(), opponent)
            } else {
                (opponent, session.metadata.tracked_team_name())
            }
        } else {
            (
                session.metadata.home.as_str(),
                session.metadata.away.as_str(),
            )
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(
                    home,
                    if session.metadata.tracked_are_home() {
                        BLUE
                    } else {
                        RED
                    },
                ),
                Span::raw(" : "),
                Span::styled(
                    away,
                    if session.metadata.tracked_are_home() {
                        RED
                    } else {
                        BLUE
                    },
                ),
            ]))
            .alignment(Alignment::Center),
            Rect::new(score.x, score.y + 3, score.width, 1),
        );
        let state = if self.time >= self.timeline.duration {
            "END"
        } else if self.paused {
            "PAUSED"
        } else {
            "PLAYING"
        };
        let mut status = vec![
            Span::raw(format!(
                "Wall {} · ",
                self.timeline.wall_time(self.time).format("%H:%M:%S")
            )),
            Span::styled(state, Style::default().fg(Color::LightYellow).bold()),
            Span::raw(format!(
                " · {} {}×",
                if self.manual_speed.is_some() {
                    "MANUAL"
                } else {
                    "AUTO"
                },
                self.speed()
            )),
        ];
        if let Some(warning) = &self.timeline.warning {
            status.push(Span::styled(format!(" · {warning}"), Color::LightYellow));
        }
        frame.render_widget(
            Paragraph::new(Line::from(status)).style(Style::default().bg(BACKGROUND)),
            info,
        );
        frame.render_widget(
            Block::default().style(Style::default().bg(BACKGROUND)),
            banner,
        );
        if let Some(cue) = self
            .cue
            .as_ref()
            .filter(|cue| now.duration_since(cue.started) < cue.duration())
        {
            arcade::draw(frame, banner, Some(cue), now, None);
        }
        self.draw_pitch(frame, pitch, session);
        self.draw_note(frame, note, session);
        self.draw_progress(frame, progress, session);
        frame.render_widget(
            Paragraph::new("Space play/pause · ←/→ 5s · ↑/↓ notes · E details · Esc menu · Q exit"),
            controls,
        );
    }

    fn draw_note(&mut self, frame: &mut Frame, area: Rect, session: &Session) {
        let title = self.note_cursor.map_or_else(
            || " NOTES ".to_owned(),
            |cursor| {
                format!(
                    " NOTE {}/{} · {}{} ",
                    cursor + 1,
                    self.timeline.notes.len(),
                    clock(self.timeline.notes[cursor].0),
                    if self.paused {
                        " · SPACE TO CONTINUE"
                    } else {
                        ""
                    }
                )
            },
        );
        let inner = presentation::solid_panel(
            frame,
            area,
            Block::default().title(title),
            Color::LightYellow,
        );
        let mut lines = Vec::new();
        if let Some(index) = self.current_entry() {
            let entry = &session.entries[index];
            lines.push(Line::raw(entry.display_text()));
            if let Some(details) = &entry.postgame {
                if details.actionable || !details.title.is_empty() {
                    lines.push(Line::raw(format!(
                        "{}{}",
                        if details.actionable {
                            "✓ ACTION · "
                        } else {
                            ""
                        },
                        details.title
                    )));
                }
                if !details.owner.is_empty() || !details.robots.is_empty() {
                    lines.push(Line::raw(format!(
                        "Volunteer: {} · Robots: {}",
                        details.owner, details.robots
                    )));
                }
                if !details.discussion.is_empty() {
                    lines.push(Line::raw(format!("Discussion: {}", details.discussion)));
                }
            }
        } else if let Some(note) = self.timeline.notes.first() {
            lines.push(Line::raw(format!("Next note · {}", clock(note.0))));
        } else {
            lines.push(Line::raw("No human notes"));
        }
        let paragraph = Paragraph::new(lines)
            .wrap(Wrap { trim: true })
            .style(Style::default().fg(Color::LightYellow));
        let maximum = paragraph
            .line_count(inner.width)
            .saturating_sub(usize::from(inner.height))
            .min(usize::from(u16::MAX)) as u16;
        self.note_scroll = self.note_scroll.min(maximum);
        frame.render_widget(paragraph.scroll((self.note_scroll, 0)), inner);
    }

    fn draw_progress(&self, frame: &mut Frame, area: Rect, session: &Session) {
        if area.width == 0 {
            return;
        }
        let width = usize::from(area.width);
        let index = |time: f64| {
            ((time / self.timeline.duration.max(1.0)).clamp(0.0, 1.0) * (width - 1) as f64).round()
                as usize
        };
        let mut bar = vec![Span::styled("━", Color::DarkGray); width];
        for item in &mut bar[..=index(self.time)] {
            *item = Span::styled("━", BLUE);
        }
        for &time in &self.markers {
            bar[index(time)] = Span::styled("│", BLUE);
        }
        for &(time, entry) in &self.timeline.events {
            if matches!(
                session.entries[entry].kind,
                EventKind::GoalHome | EventKind::GoalAway
            ) {
                bar[index(time)] = Span::styled("●", Color::White);
            }
        }
        for &(time, _) in &self.timeline.notes {
            bar[index(time)] = Span::styled("◆", Color::LightYellow);
        }
        bar[index(self.time)] = Span::styled("█", Color::White);
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(bar),
                Line::raw(format!(
                    "{} / {} · ◆ notes ● goals │ events · B balls {} · +/- speed · R auto",
                    clock(self.time),
                    clock(self.timeline.duration),
                    if self.balls { "ON" } else { "OFF" }
                )),
            ]),
            area,
        );
    }

    fn draw_pitch(&self, frame: &mut Frame, area: Rect, session: &Session) {
        let first_half = self.first_half(session);
        let tracked = session.metadata.tracked_team_number();
        let opponent = self
            .timeline
            .controller(self.time)
            .map(|sample| sample.snapshot.opponent_number)
            .or_else(|| crate::teams::team_number(session.metadata.opponent_name()));
        let mut legend = format!(
            " 1–5 own · 6–0 opponent · H {} opponents",
            if self.show_opponents { "hide" } else { "show" }
        );
        if let Some((own, player)) = self.selected_robot {
            legend.push_str(&format!(
                " · Selected {} #{player}",
                if own { "own" } else { "opponent" }
            ));
        }
        let robots = self.timeline.robots(self.time);
        let visible = |team: u8| team == tracked || (self.show_opponents && Some(team) == opponent);
        let stale = robots
            .iter()
            .filter(|(team, _, pose)| visible(*team) && pose.age > 2.0)
            .count();
        if stale > 0 {
            legend.push_str(&format!(" · {stale} stale"));
        }
        if !robots.iter().any(|(team, _, _)| visible(*team)) {
            legend.push_str(" · No robot positions");
        }
        let inner = presentation::solid_panel(
            frame,
            area,
            Block::default()
                .title(format!(
                    " {} {} ",
                    session.metadata.tracked_team_name(),
                    if first_half {
                        "ATTACK ━━━━━▶"
                    } else {
                        "◀━━━━━ ATTACK"
                    }
                ))
                .title_alignment(Alignment::Center)
                .title_bottom(legend),
            BLUE,
        );
        if inner.width == 0 || inner.height == 0 {
            return;
        }
        let Some(dimensions) = session.field_dimensions.as_ref() else {
            frame.render_widget(
                Paragraph::new("Saved field geometry unavailable")
                    .alignment(Alignment::Center)
                    .style(Style::default().fg(Color::DarkGray)),
                inner,
            );
            return;
        };
        let ratio = f64::from(inner.width) / (f64::from(inner.height) * 2.0);
        let margin = dimensions
            .border_strip_width
            .max(dimensions.goal_depth + dimensions.goal_post_diameter / 2.0);
        let x = (dimensions.length / 2.0 + margin + dimensions.line_width).max(
            (dimensions.width / 2.0 + dimensions.border_strip_width + dimensions.line_width)
                * ratio,
        );
        let y = x / ratio;
        let pixel = (2.0 * x / (f64::from(inner.width) * 2.0))
            .max(2.0 * y / (f64::from(inner.height) * 4.0));
        let canvas = Canvas::default()
            .marker(Marker::Braille)
            .background_color(Color::Rgb(5, 20, 13))
            .x_bounds([-x, x])
            .y_bounds([-y, y])
            .paint(|ctx| {
                let strokes = (dimensions.line_width / pixel).ceil().clamp(1.0, 12.0) as usize;
                for stroke in 0..strokes {
                    let offset = if strokes == 1 {
                        0.0
                    } else {
                        dimensions.line_width * (stroke as f64 / (strokes - 1) as f64 - 0.5)
                    };
                    draw_markings(ctx, dimensions, offset);
                }
                draw_goals(ctx, dimensions, first_half);
                let mut selected = None;
                for (team, player, pose) in &robots {
                    if !visible(*team) || !(1..=5).contains(player) {
                        continue;
                    }
                    let own = *team == tracked;
                    let sign = if own == first_half { 1.0 } else { -1.0 };
                    let (rx, ry) = (sign * pose.x, sign * pose.y);
                    let heading = pose.heading + if sign < 0.0 { PI } else { 0.0 };
                    let base = if own && !self.show_opponents {
                        OWN_COLORS[usize::from(*player - 1)]
                    } else if own {
                        BLUE
                    } else {
                        RED
                    };
                    let color = if pose.age > 2.0 { muted(base) } else { base };
                    triangle(ctx, rx, ry, heading, color);
                    ctx.print(
                        rx + 0.4,
                        ry + 0.4,
                        Span::styled(
                            format!("{player}{}", if pose.fallen { "×" } else { "" }),
                            color,
                        ),
                    );
                    if self.selected_robot == Some((own, *player)) {
                        selected = Some((
                            rx,
                            ry,
                            if pose.age > 2.0 {
                                Color::DarkGray
                            } else {
                                Color::Yellow
                            },
                        ));
                    }
                    if self.balls && pose.ball_age <= 5.0 {
                        if let Some((bx, by)) = pose.ball {
                            let (bx, by) = (sign * bx, sign * by);
                            let color = if pose.ball_age > 2.0 {
                                muted(base)
                            } else {
                                color
                            };
                            ctx.draw(&Circle {
                                x: bx,
                                y: by,
                                radius: 0.12_f64.max(pixel),
                                color,
                            });
                            clipped_line(ctx, (rx, ry), (bx, by), (x, y), color);
                        }
                    }
                }
                if let Some((rx, ry, color)) = selected {
                    ctx.layer();
                    ctx.draw(&Circle {
                        x: rx,
                        y: ry,
                        radius: 0.8,
                        color,
                    });
                }
            });
        frame.render_widget(canvas, inner);
        presentation::grass_stripes(
            frame,
            inner,
            [dimensions.length, dimensions.width],
            [2.0 * x, 2.0 * y],
            Direction::Vertical,
        );
    }
}

fn clock(seconds: f64) -> String {
    format!("{:02}:{:02}", seconds as u64 / 60, seconds as u64 % 60)
}

fn muted(color: Color) -> Color {
    match color {
        Color::LightCyan => Color::Rgb(70, 115, 115),
        Color::LightGreen => Color::Rgb(70, 115, 70),
        Color::LightYellow => Color::Rgb(115, 115, 70),
        Color::LightMagenta => Color::Rgb(115, 70, 115),
        Color::LightBlue => Color::Rgb(75, 85, 120),
        Color::LightRed => Color::Rgb(120, 70, 70),
        Color::Rgb(r, g, b) => Color::Rgb(r / 2, g / 2, b / 2),
        _ => Color::DarkGray,
    }
}

fn triangle(ctx: &mut Context<'_>, x: f64, y: f64, heading: f64, color: Color) {
    let points = [heading, heading + PI * 0.78, heading - PI * 0.78]
        .map(|angle| (x + angle.cos() * 0.43, y + angle.sin() * 0.43));
    for index in 0..3 {
        let (a, b) = (points[index], points[(index + 1) % 3]);
        ctx.draw(&Segment::new(a.0, a.1, b.0, b.1, color));
    }
}

fn clipped_line(
    ctx: &mut Context<'_>,
    from: (f64, f64),
    to: (f64, f64),
    bounds: (f64, f64),
    color: Color,
) {
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let (mut start, mut end) = (0.0_f64, 1.0_f64);
    for (origin, delta, bound) in [(from.0, dx, bounds.0), (from.1, dy, bounds.1)] {
        if delta == 0.0 {
            if origin.abs() > bound {
                return;
            }
        } else {
            let (a, b) = ((-bound - origin) / delta, (bound - origin) / delta);
            start = start.max(a.min(b));
            end = end.min(a.max(b));
        }
    }
    if start <= end {
        ctx.draw(&Segment::new(
            (from.0 + start * dx).clamp(-bounds.0, bounds.0),
            (from.1 + start * dy).clamp(-bounds.1, bounds.1),
            (from.0 + end * dx).clamp(-bounds.0, bounds.0),
            (from.1 + end * dy).clamp(-bounds.1, bounds.1),
            color,
        ));
    }
}

fn draw_markings(ctx: &mut Context<'_>, d: &FieldDimensions, offset: f64) {
    let color = Color::Rgb(146, 176, 153);
    let (half_length, half_width) = (d.length / 2.0, d.width / 2.0);
    ctx.draw(&Rectangle {
        x: -half_length - offset,
        y: -half_width - offset,
        width: d.length + 2.0 * offset,
        height: d.width + 2.0 * offset,
        color,
    });
    ctx.draw(&Segment::new(
        offset,
        -half_width,
        offset,
        half_width,
        color,
    ));
    ctx.draw(&Circle {
        x: 0.0,
        y: 0.0,
        radius: d.center_circle_diameter / 2.0 + offset,
        color,
    });
    for side in [-1.0, 1.0] {
        for (depth, width) in [
            (d.goal_box_area_length, d.goal_box_area_width),
            (d.penalty_area_length, d.penalty_area_width),
        ] {
            ctx.draw(&Rectangle {
                x: if side < 0.0 {
                    -half_length - offset
                } else {
                    half_length - depth - offset
                },
                y: -width / 2.0 - offset,
                width: depth + 2.0 * offset,
                height: width + 2.0 * offset,
                color,
            });
        }
    }
    let radius = d.penalty_marker_size / 2.0;
    for x in [
        0.0,
        half_length - d.penalty_marker_distance,
        -half_length + d.penalty_marker_distance,
    ] {
        ctx.draw(&Segment::new(x - radius, offset, x + radius, offset, color));
        ctx.draw(&Segment::new(
            x + offset,
            -radius,
            x + offset,
            radius,
            color,
        ));
    }
    if d.corner_arc_radius > 0.0 {
        let radius = (d.corner_arc_radius + offset).max(0.0);
        for side_x in [-1.0, 1.0] {
            for side_y in [-1.0, 1.0] {
                let points: Vec<_> = (0..=90)
                    .map(|angle| {
                        let (sin, cos) = f64::from(angle).to_radians().sin_cos();
                        (
                            side_x * (half_length - radius * cos),
                            side_y * (half_width - radius * sin),
                        )
                    })
                    .collect();
                ctx.draw(&Points {
                    coords: &points,
                    color,
                });
            }
        }
    }
}

fn draw_goals(ctx: &mut Context<'_>, d: &FieldDimensions, first_half: bool) {
    let post = (d.goal_inner_width + d.goal_post_diameter) / 2.0;
    for side in [-1.0, 1.0] {
        let goal_line = side * d.length / 2.0;
        ctx.draw(&Rectangle {
            x: if side < 0.0 {
                goal_line - d.goal_depth
            } else {
                goal_line
            },
            y: -post,
            width: d.goal_depth,
            height: post * 2.0,
            color: if (side < 0.0) == first_half {
                BLUE
            } else {
                RED
            },
        });
        for y in [-post, post] {
            ctx.draw(&Circle {
                x: goal_line,
                y,
                radius: d.goal_post_diameter / 2.0,
                color: Color::White,
            });
        }
    }
}
