use std::{io, path::Path};

use chrono::{DateTime, FixedOffset, Local, NaiveDate};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{style::Color, widgets::ListState};
use tui_textarea::{CursorMove, TextArea};

use crate::{
    capture::{CaptureLog, Packet},
    discovery::Discovery,
    game_controller::{Receiver, Update, format_clock},
    model::{Entry, EventKind, MatchType, Metadata, Session},
    navigation::{Command, Keybindings, wrap_selection},
    presentation::{ACCENT, ERROR, FOCUS},
    settings::Settings,
    storage::Store,
    teams::{TEAMS, team_name, team_number},
};

pub const EVENT_KINDS: [EventKind; 9] = [
    EventKind::GameStart,
    EventKind::HalfTime,
    EventKind::SecondHalfStart,
    EventKind::GoalHome,
    EventKind::GoalAway,
    EventKind::GameEnd,
    EventKind::Custom,
    EventKind::Note,
    EventKind::GoldenGoalStart,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Mark(EventKind),
    Match,
    Correct,
    Delete,
    Restore,
    Controller,
    TrueData,
    PreviewUp,
    PreviewDown,
    RetrySave,
    Undo,
    Redo,
    Quit,
    OpenLogFolder,
    ReconnectController,
}

impl Action {
    pub fn label(self) -> &'static str {
        match self {
            Self::Mark(EventKind::Note) => "New note",
            Self::Mark(kind) => kind.label(),
            Self::Match => "Edit match details",
            Self::Correct => "Correct selected event",
            Self::Delete => "Remove selected event (reversible)",
            Self::Restore => "Restore removed event",
            Self::Controller => "Resume GC logging",
            Self::TrueData => "Request true GC data",
            Self::PreviewUp => "Scroll preview up",
            Self::PreviewDown => "Scroll preview down",
            Self::RetrySave => "Retry saving",
            Self::Undo => "Undo note edit",
            Self::Redo => "Redo note edit",
            Self::Quit => "Save and quit",
            Self::OpenLogFolder => "Open log folder",
            Self::ReconnectController => "Reconnect/switch GC (same match)",
        }
    }
}

pub const ACTIONS: &[Action] = &[
    Action::Mark(EventKind::GameStart),
    Action::Mark(EventKind::HalfTime),
    Action::Mark(EventKind::SecondHalfStart),
    Action::Mark(EventKind::GoalHome),
    Action::Mark(EventKind::GoalAway),
    Action::Mark(EventKind::Custom),
    Action::Mark(EventKind::Note),
    Action::Mark(EventKind::GoldenGoalStart),
    Action::Match,
    Action::Correct,
    Action::Delete,
    Action::Restore,
    Action::Controller,
    Action::TrueData,
    Action::ReconnectController,
    Action::PreviewUp,
    Action::PreviewDown,
    Action::RetrySave,
    Action::Undo,
    Action::Redo,
    Action::OpenLogFolder,
    Action::Mark(EventKind::GameEnd),
    Action::Quit,
];

pub enum Mode {
    Idle,
    Writing,
    Actions {
        selected: usize,
        timestamp: DateTime<FixedOffset>,
        message_budget: Option<u16>,
        gc_seconds_remaining: Option<i16>,
    },
    Form(Form),
    ReconnectController {
        selected: usize,
    },
    Delete {
        index: usize,
        selected: usize,
    },
}

#[derive(Clone, Copy)]
pub enum FormKind {
    Setup,
    Metadata,
    Entry(usize),
}

pub struct Field {
    pub label: &'static str,
    pub editor: TextArea<'static>,
    initial_value: String,
}

pub struct DetectedTeams {
    pub teams: [&'static str; 2],
    pub selected: usize,
}

pub struct Form {
    pub kind: FormKind,
    pub fields: Vec<Field>,
    pub selected: usize,
    pub editing: bool,
    pub discard_selected: Option<usize>,
    pub match_type: Option<MatchType>,
    pub team_selection: Option<usize>,
    pub team_filter: String,
    pub team_searching: bool,
    pub detected_teams: Option<DetectedTeams>,
    // Display tracked team first without changing saved GoalHome/GoalAway meaning.
    tracked_are_home: bool,
    legacy_teams: bool,
}

fn editor(text: &str) -> TextArea<'static> {
    let mut editor = TextArea::new(vec![text.to_owned()]);
    editor.move_cursor(CursorMove::End);
    editor
}

impl Field {
    fn new(label: &'static str, value: &str) -> Self {
        Self {
            label,
            editor: editor(value),
            initial_value: value.to_owned(),
        }
    }

    pub fn value(&self) -> &str {
        &self.editor.lines()[0]
    }
}

impl Form {
    fn metadata(kind: FormKind, metadata: &Metadata, output: &str) -> Self {
        let mut fields = vec![
            Field::new("Event / competition", &metadata.event),
            Field::new("Tracked team", metadata.tracked_team_name()),
            Field::new("Opponent", metadata.opponent_name()),
            Field::new("Date (YYYY-MM-DD)", &metadata.date.to_string()),
            Field::new("Match label / filename", &metadata.label),
        ];
        for (label, robot) in [
            "Player 1 → robot",
            "Player 2 → robot",
            "Player 3 → robot",
            "Player 4 → robot",
            "Player 5 → robot",
        ]
        .into_iter()
        .zip(&metadata.robots)
        {
            fields.push(Field::new(label, robot));
        }
        let setup = matches!(kind, FormKind::Setup);
        if setup {
            fields.push(Field::new("Output directory", output));
            fields.push(Field::new("Match type", "Golden Goal + official"));
        }
        Self {
            kind,
            fields,
            selected: 0,
            editing: false,
            discard_selected: None,
            match_type: setup.then_some(MatchType::Combined),
            team_selection: None,
            team_filter: String::new(),
            team_searching: false,
            detected_teams: None,
            tracked_are_home: metadata.tracked_are_home(),
            legacy_teams: !setup && metadata.tracked_team.is_none(),
        }
    }

    fn entry(index: usize, entry: &Entry) -> Self {
        Self {
            kind: FormKind::Entry(index),
            fields: vec![
                Field::new("Event type", entry.kind.label()),
                Field::new(
                    "Timestamp (RFC3339, explicitly editable)",
                    &entry.timestamp.to_rfc3339(),
                ),
                Field::new("Description", &entry.text),
                Field::new(
                    "GC countdown (MM:SS; blank = unavailable)",
                    &entry
                        .gc_seconds_remaining
                        .map(format_clock)
                        .unwrap_or_default(),
                ),
            ],
            selected: 2,
            editing: false,
            discard_selected: None,
            match_type: None,
            team_selection: None,
            team_filter: String::new(),
            team_searching: false,
            detected_teams: None,
            tracked_are_home: true,
            legacy_teams: false,
        }
    }

    fn changed(&self) -> bool {
        self.fields
            .iter()
            .any(|field| field.value() != field.initial_value)
    }

    pub fn read_metadata(&self) -> Result<Metadata, String> {
        let tracked = self.fields[1].value().trim();
        let opponent = self.fields[2].value().trim();
        let tracked_number = team_number(tracked).ok_or("Choose a known tracked team")?;
        let opponent_number = team_number(opponent).ok_or("Choose a known opponent")?;
        if tracked_number == opponent_number {
            return Err("Tracked team and opponent must be different".into());
        }
        let legacy_unchanged = self.legacy_teams
            && team_number(&self.fields[1].initial_value) == Some(tracked_number)
            && team_number(&self.fields[2].initial_value) == Some(opponent_number);
        let (home, away) = if self.tracked_are_home {
            (tracked, opponent)
        } else {
            (opponent, tracked)
        };
        let metadata = Metadata {
            event: self.fields[0].value().trim().to_owned(),
            home: home.to_owned(),
            away: away.to_owned(),
            tracked_team: (!legacy_unchanged).then_some(tracked_number),
            date: NaiveDate::parse_from_str(self.fields[3].value(), "%Y-%m-%d")
                .map_err(|_| "Date must be YYYY-MM-DD".to_owned())?,
            label: self.fields[4].value().trim().to_owned(),
            robots: std::array::from_fn(|i| self.fields[5 + i].value().trim().to_owned()),
        };
        metadata.validate()?;
        if self.match_type == Some(MatchType::GoldenGoal) && opponent_number != 0 {
            return Err("Golden Goal opponent must be Invisibles; use a new official session for a real match".into());
        }
        Ok(metadata)
    }

    pub fn read_entry(&self, original: &Entry) -> Result<Entry, String> {
        let kind = EVENT_KINDS
            .into_iter()
            .find(|kind| kind.label() == self.fields[0].value())
            .ok_or("Unknown event type")?;
        let kind = match_event_kind(self.match_type, kind)?;
        let timestamp = DateTime::parse_from_rfc3339(self.fields[1].value())
            .map_err(|_| "Timestamp must include date, time and timezone (RFC3339)".to_owned())?;
        Ok(Entry {
            kind,
            timestamp,
            text: self.fields[2].value().to_owned(),
            gc_seconds_remaining: parse_gc_clock(self.fields[3].value())?,
            score_delta: if kind == original.kind {
                original.score_delta
            } else {
                None
            },
            ..original.clone()
        })
    }

    pub(crate) fn is_team_field(&self) -> bool {
        !matches!(self.kind, FormKind::Entry(_)) && matches!(self.selected, 1 | 2)
    }

    pub(crate) fn team_choices(&self) -> &'static [(u8, &'static str)] {
        if self.selected == 2 && self.match_type == Some(MatchType::GoldenGoal) {
            &TEAMS[..1]
        } else {
            TEAMS
        }
    }

    pub(crate) fn visible_teams(&self) -> Vec<usize> {
        let filter = self.team_filter.to_lowercase();
        self.team_choices()
            .iter()
            .enumerate()
            .filter_map(|(index, (_, name))| name.to_lowercase().contains(&filter).then_some(index))
            .collect()
    }

    fn update_team_filter(&mut self) {
        let visible = self.visible_teams();
        if !self
            .team_selection
            .is_some_and(|index| visible.contains(&index))
        {
            self.team_selection = Some(visible.first().copied().unwrap_or(0));
        }
    }

    pub(crate) fn is_choice_field(&self) -> bool {
        self.is_team_field()
            || (matches!(self.kind, FormKind::Entry(_)) && self.selected == 0)
            || (matches!(self.kind, FormKind::Setup) && self.selected == 11)
    }

    fn navigate(&mut self, command: Command) {
        if let Some(detected) = &mut self.detected_teams {
            match command {
                Command::Up | Command::Left | Command::Down | Command::Right => {
                    detected.selected = wrap_selection(
                        detected.selected,
                        3,
                        matches!(command, Command::Down | Command::Right),
                    );
                }
                Command::Confirm if detected.selected > 0 => {
                    let tracked = detected.selected - 1;
                    self.fields[1].editor = editor(detected.teams[tracked]);
                    self.fields[2].editor = editor(detected.teams[1 - tracked]);
                    self.match_type = Some(MatchType::Official);
                    self.fields[11].editor = editor("Official match");
                    if self.fields[4].value() == "Golden_Goal" {
                        self.fields[4].editor = editor("1st_game");
                    }
                    self.detected_teams = None;
                }
                Command::Confirm | Command::Back => self.detected_teams = None,
                _ => {}
            }
            return;
        }
        if let Some(selected) = self.team_selection {
            let visible = self.visible_teams();
            match command {
                Command::Up | Command::Left | Command::Down | Command::Right => {
                    if let Some(position) = visible.iter().position(|index| *index == selected) {
                        self.team_selection = Some(
                            visible[wrap_selection(
                                position,
                                visible.len(),
                                matches!(command, Command::Down | Command::Right),
                            )],
                        );
                    }
                }
                Command::Confirm if self.team_searching => self.team_searching = false,
                Command::Confirm if visible.contains(&selected) => {
                    self.fields[self.selected].editor = editor(self.team_choices()[selected].1);
                    self.team_selection = None;
                    self.team_filter.clear();
                }
                Command::Back if self.team_searching || !self.team_filter.is_empty() => {
                    self.team_searching = false;
                    self.team_filter.clear();
                }
                Command::Back => self.team_selection = None,
                _ => {}
            }
            return;
        }
        let controls = self.fields.len() + 2 + usize::from(matches!(self.kind, FormKind::Setup));
        match command {
            Command::Down => self.selected = (self.selected + 1) % controls,
            Command::Up => self.selected = (self.selected + controls - 1) % controls,
            Command::Confirm if self.selected < self.fields.len() => {
                if self.is_team_field() {
                    let number = team_number(self.fields[self.selected].value());
                    self.team_selection = Some(
                        self.team_choices()
                            .iter()
                            .position(|(id, _)| Some(*id) == number)
                            .unwrap_or(0),
                    );
                } else if self.is_choice_field() {
                    self.navigate(Command::Right);
                } else {
                    self.editing = true;
                }
            }
            Command::Left | Command::Right
                if matches!(self.kind, FormKind::Setup) && self.selected == 11 =>
            {
                let choices = [
                    MatchType::Combined,
                    MatchType::GoldenGoal,
                    MatchType::Official,
                ];
                let current = choices
                    .iter()
                    .position(|choice| Some(*choice) == self.match_type)
                    .unwrap_or(0);
                let step = if command == Command::Left {
                    choices.len() - 1
                } else {
                    1
                };
                let selected = choices[(current + step) % choices.len()];
                self.match_type = Some(selected);
                self.fields[11].editor = editor(match selected {
                    MatchType::Combined => "Golden Goal + official",
                    MatchType::GoldenGoal => "Golden Goal",
                    MatchType::Official => "Official match",
                });
                if selected == MatchType::GoldenGoal {
                    self.fields[2].editor = editor("Invisibles");
                    if self.fields[4].value() == "1st_game" {
                        self.fields[4].editor = editor("Golden_Goal");
                    }
                } else if self.fields[4].value() == "Golden_Goal" {
                    self.fields[4].editor = editor("1st_game");
                }
            }
            Command::Left | Command::Right
                if matches!(self.kind, FormKind::Entry(_)) && self.selected == 0 =>
            {
                let kinds: Vec<_> = EVENT_KINDS
                    .into_iter()
                    .filter(|kind| match_event_kind(self.match_type, *kind) == Ok(*kind))
                    .collect();
                let current = kinds
                    .iter()
                    .position(|kind| kind.label() == self.fields[0].value())
                    .unwrap_or(0);
                let step = if command == Command::Left {
                    kinds.len() - 1
                } else {
                    1
                };
                self.fields[0].editor = editor(kinds[(current + step) % kinds.len()].label());
            }
            Command::Left => self.selected = (self.selected + controls - 1) % controls,
            Command::Right => self.selected = (self.selected + 1) % controls,
            _ => {}
        }
    }
}

pub struct App {
    pub session: Option<Session>,
    pub store: Option<Store>,
    pub mode: Mode,
    pub keys: Keybindings,
    pub help_visible: bool,
    pub controls_visible: bool,
    pub note_editor: TextArea<'static>,
    pub history: ListState,
    pub preview_scroll: Option<u16>,
    pub preview_max: u16,
    pub status: String,
    pub save_error: Option<String>,
    pub dirty: bool,
    pub controller: Option<Receiver>,
    pub discovery: Discovery,
    pub controller_error: Option<String>,
    pub pending_controller: Vec<Update>,
    pub capture_log: Option<CaptureLog>,
    pub pending_capture: Vec<Packet>,
    pub last_capture_id: Option<u64>,
    action_capture_after: Option<u64>,
}

impl App {
    #[cfg(test)]
    pub fn new(path: Option<&Path>) -> io::Result<Self> {
        Self::new_with_settings(path, &Settings::default())
    }

    pub fn new_with_settings(path: Option<&Path>, settings: &Settings) -> io::Result<Self> {
        if let Some(path) = path {
            let (store, mut session) = Store::open(path)?;
            let capture_log = if session.capture_session.is_some() {
                Some(store.prepare_capture(&mut session)?)
            } else {
                None
            };
            let last_capture_id = capture_log
                .as_ref()
                .and_then(|log| log.next_id().checked_sub(1));
            let mut app = Self {
                session: Some(session),
                store: Some(store),
                mode: Mode::Idle,
                keys: settings.keys.clone(),
                help_visible: false,
                controls_visible: true,
                note_editor: editor(""),
                history: ListState::default(),
                preview_scroll: None,
                preview_max: 0,
                status: "Session reopened".into(),
                save_error: None,
                dirty: false,
                controller: None,
                discovery: Discovery::default(),
                controller_error: None,
                pending_controller: Vec::new(),
                capture_log,
                pending_capture: Vec::new(),
                last_capture_id,
                action_capture_after: None,
            };
            app.restore_mode();
            if let Some(index) = app.session.as_ref().and_then(|session| session.active_note) {
                app.history.select(Some(index));
            } else {
                app.select_latest();
            }
            return Ok(app);
        }
        let metadata = Metadata {
            event: settings.event.clone(),
            home: settings.home.clone(),
            away: settings.away.clone(),
            tracked_team: team_number(&settings.home),
            date: Local::now().date_naive(),
            label: "1st_game".into(),
            robots: settings.robots.clone(),
        };
        Ok(Self {
            session: None,
            store: None,
            mode: Mode::Form(Form::metadata(
                FormKind::Setup,
                &metadata,
                &settings.log_directory.to_string_lossy(),
            )),
            keys: settings.keys.clone(),
            help_visible: false,
            controls_visible: true,
            note_editor: editor(""),
            history: ListState::default(),
            preview_scroll: None,
            preview_max: 0,
            status: "Select a field, confirm to edit, then select Create match".into(),
            save_error: None,
            dirty: false,
            controller: None,
            discovery: Discovery::default(),
            controller_error: None,
            pending_controller: Vec::new(),
            capture_log: None,
            pending_capture: Vec::new(),
            last_capture_id: None,
            action_capture_after: None,
        })
    }

    pub fn offer_detected_teams(&mut self, teams: Option<(u8, u8)>) {
        let Mode::Form(form) = &mut self.mode else {
            return;
        };
        if !matches!(form.kind, FormKind::Setup) {
            return;
        }
        let teams = teams.and_then(|(first, second)| Some([team_name(first)?, team_name(second)?]));
        if let Some(teams) = teams {
            form.detected_teams = Some(DetectedTeams { teams, selected: 0 });
            self.status = "Choose the tracked team to set both draft teams and use Official match type; Cancel keeps the draft".into();
        } else {
            self.status =
                "No fresh supported GC teams detected; fill in the setup manually or try again"
                    .into();
        }
    }

    pub fn poll_discovery(&mut self) {
        if self.controller.is_none() {
            self.discovery.poll();
        } else {
            self.discovery = Discovery::default();
        }
    }

    pub fn mode_style(&self) -> (&'static str, Color) {
        if self.save_error.is_some() {
            return ("SAVE FAILED — CHANGES IN MEMORY", ERROR);
        }
        match &self.mode {
            Mode::Idle => ("IDLE — ENTER TO MARK AN EVENT", ACCENT),
            Mode::Writing => ("WRITING — TIMESTAMP CAPTURED", FOCUS),
            Mode::Actions { .. } => ("ACTION MENU — TIME CAPTURED", ACCENT),
            Mode::Form(form) => match form.kind {
                FormKind::Setup => ("MATCH SETUP — NOT CREATED YET", ACCENT),
                FormKind::Metadata => ("EDIT MATCH — SELECT SAVE", ACCENT),
                FormKind::Entry(_) => ("CORRECTION — SELECT SAVE", ACCENT),
            },
            Mode::Delete { .. } => ("REMOVE EVENT — CAN BE RESTORED", FOCUS),
            Mode::ReconnectController { .. } => ("RECONNECT GC — SAME MATCH", FOCUS),
        }
    }

    pub fn persist(&mut self) {
        if !self.pending_capture.is_empty() {
            let result = self
                .capture_log
                .as_mut()
                .ok_or_else(|| io::Error::other("Raw capture is not open"))
                .and_then(|log| log.append(&self.pending_capture));
            if let Err(error) = result {
                self.dirty = true;
                self.save_error = Some(format!(
                    "Raw capture save failed: {error}. Select Retry saving in Actions; quitting is blocked."
                ));
                return;
            }
            self.pending_capture.clear();
            self.save_error = None;
        }
        if !self.dirty {
            return;
        }
        if let (Some(store), Some(session)) = (&self.store, &self.session) {
            match store.save(session) {
                Ok(()) => {
                    self.dirty = false;
                    self.save_error = None;
                }
                Err(error) => {
                    self.save_error = Some(format!(
                        "Save failed: {error}. Select Retry saving in Actions; quitting is blocked."
                    ))
                }
            }
        }
    }

    fn restore_mode(&mut self) {
        let active = self.session.as_ref().and_then(|session| {
            session
                .active_note
                .map(|index| (index, &session.entries[index]))
        });
        if let Some((_, entry)) = active {
            if self.note_editor.lines().join("\n") != entry.text {
                self.note_editor = editor(&entry.text);
            }
            self.mode = Mode::Writing;
        } else {
            self.mode = Mode::Idle;
        }
    }

    fn select_latest(&mut self) {
        self.history.select(
            self.session
                .as_ref()
                .and_then(|session| session.entries.len().checked_sub(1)),
        );
    }

    fn capture_anchor(&self) -> Option<u64> {
        self.last_capture_id.or_else(|| {
            self.capture_log
                .as_ref()
                .and_then(|log| log.next_id().checked_sub(1))
        })
    }

    fn gc_values(&self) -> (Option<u16>, Option<i16>) {
        self.controller
            .as_ref()
            .filter(|controller| !controller.is_stale())
            .and_then(|controller| controller.latest.as_ref())
            .map_or((None, None), |packet| {
                (
                    Some(packet.tracked_message_budget),
                    Some(packet.seconds_remaining),
                )
            })
    }

    fn record(
        &mut self,
        kind: EventKind,
        now: DateTime<FixedOffset>,
        message_budget: Option<u16>,
        gc_seconds_remaining: Option<i16>,
        capture_after: Option<u64>,
    ) {
        let start_official = self.session.as_ref().is_some_and(|session| {
            session.match_type == Some(MatchType::Combined)
                && session.current_game() == MatchType::GoldenGoal
                && session.is_finished()
                && kind == EventKind::GameStart
        });
        if start_official && self.controller.is_some() {
            self.status = "Waiting for GC official restart / opponent change. Stop listening to start the official game manually.".into();
            self.restore_mode();
            return;
        }
        let match_type = self.session.as_ref().and_then(|session| {
            session.match_type.map(|match_type| {
                if match_type == MatchType::Combined {
                    if start_official {
                        MatchType::Official
                    } else {
                        session.current_game()
                    }
                } else {
                    match_type
                }
            })
        });
        let kind = match match_event_kind(match_type, kind) {
            Ok(kind) => kind,
            Err(error) => {
                self.status = error;
                self.restore_mode();
                return;
            }
        };
        if self
            .session
            .as_ref()
            .is_some_and(|session| session.match_type.is_some() && session.is_finished())
            && !start_official
            && !matches!(kind, EventKind::Note | EventKind::Custom)
        {
            self.status =
                "Game finished. Remove its Game end marker to resume, or create a new session."
                    .into();
            self.restore_mode();
            return;
        }
        if matches!(kind, EventKind::GoalHome | EventKind::GoalAway)
            && self.controller.is_some()
            && self
                .session
                .as_ref()
                .is_some_and(|session| session.match_type.is_some() || !session.in_golden_goal())
        {
            self.status =
                "GC owns goals while logging. Select Pause GC logging in Actions for manual goals."
                    .into();
            self.restore_mode();
            return;
        }
        if let Some(session) = &mut self.session {
            let legacy_golden_goal = session.match_type.is_none() && session.in_golden_goal();
            if kind == EventKind::Note {
                // A second event can interrupt a draft without losing its text or timestamp.
                session.submit_note();
                session.start_note(now);
                self.note_editor = editor("");
            } else {
                session.record(kind, now);
            }
            let entry = session.entries.last_mut().unwrap();
            if start_official {
                entry.game = Some(MatchType::Official);
            }
            entry.capture_after = capture_after;
            entry.message_budget = message_budget;
            entry.gc_seconds_remaining = gc_seconds_remaining;
            entry.golden_goal_message_budget = if legacy_golden_goal {
                message_budget
            } else {
                None
            };
            let marked_at = if session.match_type.is_some() {
                gc_seconds_remaining.map_or_else(
                    || "GC time unavailable".into(),
                    |seconds| format!("GC {}", format_clock(seconds)),
                )
            } else {
                now.format("%H:%M:%S%.3f").to_string()
            };
            self.select_latest();
            self.preview_scroll = None;
            self.dirty = true;
            self.status = format!("{} marked at {}", kind.label(), marked_at);
        }
    }

    fn move_selection(&mut self, down: bool) {
        let len = self
            .session
            .as_ref()
            .map_or(0, |session| session.entries.len());
        if len == 0 {
            return;
        }
        let selected = self.history.selected().unwrap_or(len - 1);
        self.history.select(Some(if down {
            (selected + 1).min(len - 1)
        } else {
            selected.saturating_sub(1)
        }));
    }

    fn request_quit(&mut self) -> bool {
        if matches!(&self.mode, Mode::Form(form) if !matches!(form.kind, FormKind::Setup) || form.changed())
        {
            self.status = "Select Save or Cancel before quitting".into();
            return false;
        }
        self.poll_controller();
        self.flush_controller();
        self.persist();
        self.save_error.is_none()
    }

    fn reconnect_controller(&mut self) {
        self.poll_controller();
        self.flush_controller();
        self.persist();
        if self.save_error.is_some() {
            self.status = "GC unchanged: retry saving before reconnecting".into();
            return;
        }
        let opponent = self
            .controller
            .as_ref()
            .and_then(Receiver::opponent_number)
            .or_else(|| {
                self.session.as_ref().and_then(|session| {
                    if session.current_game() == MatchType::Official {
                        session.official_opponent
                    } else {
                        session.golden_goal_opponent
                    }
                })
            })
            .or_else(|| {
                self.session.as_ref().and_then(|session| {
                    team_number(if session.current_game() == MatchType::GoldenGoal {
                        let (home, away) = session.current_teams();
                        if session.metadata.tracked_are_home() {
                            away
                        } else {
                            home
                        }
                    } else {
                        session.metadata.opponent_name()
                    })
                })
            });
        match (&mut self.controller, opponent) {
            (Some(controller), Some(opponent)) => {
                controller.reconnect(opponent);
                self.controller_error = None;
                self.status = format!(
                    "GC reacquiring {}; history/capture preserved. First matching controller wins; another live matching GC may be selected.",
                    crate::teams::display_name(opponent)
                );
            }
            (None, _) => self.status = "Resume GC logging first".into(),
            (_, None) => {
                self.status = "Cannot safely reconnect: current opponent is unknown".into()
            }
        }
    }

    fn scroll_preview(&mut self, down: bool) {
        let current = self.preview_scroll.unwrap_or(self.preview_max);
        let scroll = if down {
            current.saturating_add(10).min(self.preview_max)
        } else {
            current.saturating_sub(10)
        };
        self.preview_scroll = (scroll < self.preview_max).then_some(scroll);
    }

    fn perform_action(&mut self, action: Action, now: DateTime<FixedOffset>) -> bool {
        let capture_after = if matches!(self.mode, Mode::Actions { .. }) {
            self.action_capture_after
        } else {
            self.capture_anchor()
        };
        let (message_budget, gc_seconds_remaining) = match &self.mode {
            Mode::Actions {
                message_budget,
                gc_seconds_remaining,
                ..
            } => (*message_budget, *gc_seconds_remaining),
            _ => self.gc_values(),
        };
        if !matches!(action, Action::PreviewUp | Action::PreviewDown) {
            self.restore_mode();
        }
        match action {
            Action::Mark(kind) => {
                self.record(
                    kind,
                    now,
                    message_budget,
                    gc_seconds_remaining,
                    capture_after,
                );
                self.restore_mode();
                if kind == EventKind::Custom {
                    self.open_correction();
                }
            }
            Action::Match => {
                if let Some(session) = &self.session {
                    let mut form = Form::metadata(FormKind::Metadata, &session.metadata, "");
                    form.match_type = session.match_type;
                    self.mode = Mode::Form(form);
                    self.status = "Output filename stays fixed after creation; editing metadata updates the log heading".into();
                }
            }
            Action::Correct => self.open_correction(),
            Action::Delete => {
                if let (Some(session), Some(index)) = (&self.session, self.history.selected()) {
                    if session.entries[index].undone {
                        self.status =
                            "Event already removed; select Restore removed event in Actions".into();
                    } else {
                        self.mode = Mode::Delete { index, selected: 0 };
                    }
                }
            }
            Action::Restore => {
                if let (Some(session), Some(index)) = (&mut self.session, self.history.selected()) {
                    session.restore(index);
                    self.dirty = true;
                    self.status = "Event restored.".into();
                }
            }
            Action::Controller => {
                self.controller_error = None;
                if self.controller.take().is_some() {
                    self.status = "GC listening stopped; manual goals enabled".into();
                } else if self.session.is_some() {
                    self.persist();
                    if self.save_error.is_some() {
                        return false;
                    }
                    if self.capture_log.is_none() {
                        let result = match (&self.store, &mut self.session) {
                            (Some(store), Some(session)) => store.prepare_capture(session),
                            _ => Err(io::Error::other("Create a saved session before listening")),
                        };
                        match result {
                            Ok(log) => {
                                self.last_capture_id = log.next_id().checked_sub(1);
                                self.capture_log = Some(log);
                            }
                            Err(error) => {
                                self.dirty = true;
                                self.controller_error =
                                    Some(format!("Cannot start raw capture: {error}"));
                                return false;
                            }
                        }
                    }
                    let metadata = &self.session.as_ref().unwrap().metadata;
                    let expected_opponent = metadata
                        .tracked_team
                        .and_then(|_| team_number(metadata.opponent_name()));
                    // Discovery and match logging share UDP 3838; hand off the socket.
                    self.discovery = Discovery::default();
                    match Receiver::bind(metadata.tracked_team_number(), expected_opponent) {
                        Ok(mut receiver) => {
                            let session = self.session.as_ref().unwrap();
                            receiver.awaiting_official_start = session.match_type
                                == Some(MatchType::Combined)
                                && session.current_game() == MatchType::Official
                                && session.official_opponent.is_some()
                                && session.official_opponent == session.golden_goal_opponent
                                && !session.entries.iter().any(|entry| {
                                    !entry.undone
                                        && entry.game == Some(MatchType::Official)
                                        && matches!(
                                            entry.kind,
                                            EventKind::GameStart
                                                | EventKind::HalfTime
                                                | EventKind::SecondHalfStart
                                                | EventKind::GameEnd
                                        )
                                });
                            receiver.enable_capture(self.capture_log.as_ref().unwrap().next_id());
                            self.controller = Some(receiver);
                            self.status = if self
                                .session
                                .as_ref()
                                .is_some_and(|session| session.match_type.is_some())
                            {
                                "GC detection on UDP 3838; true data requested automatically; goals automatic"
                                    .into()
                            } else {
                                "GC detection on UDP 3838; true data automatic; legacy official goals automatic, test goals manual".into()
                            };
                        }
                        Err(error) => {
                            self.controller_error =
                                Some(format!("Cannot listen on UDP 3838: {error}"));
                        }
                    }
                }
            }
            Action::ReconnectController => {
                if self.controller.is_some() {
                    self.mode = Mode::ReconnectController { selected: 0 };
                } else {
                    self.status = "Resume GC logging first".into();
                }
            }
            Action::TrueData => {
                self.flush_controller();
                if let Some(controller) = &mut self.controller {
                    let enabled = !controller.true_data;
                    match controller.set_true_data(enabled) {
                        Ok(()) => {
                            self.controller_error = None;
                            self.status = if enabled {
                                "True GC data requested; waiting for response, without broadcast fallback".into()
                            } else {
                                "Using GC broadcasts; waiting for a fresh packet".into()
                            };
                        }
                        Err(error) => {
                            self.controller_error =
                                Some(format!("Cannot request true GC data: {error}"))
                        }
                    }
                } else {
                    self.status =
                        "Select Resume GC logging in Actions first; wait for detection, then request true data"
                            .into();
                }
            }
            Action::RetrySave => {
                self.dirty = true;
                self.persist();
            }
            Action::Undo | Action::Redo => {
                if matches!(self.mode, Mode::Writing) {
                    let changed = if matches!(action, Action::Undo) {
                        self.note_editor.undo()
                    } else {
                        self.note_editor.redo()
                    };
                    if changed {
                        self.sync_note();
                    }
                }
            }
            Action::Quit => return self.request_quit(),
            Action::OpenLogFolder => {
                self.status = match &self.store {
                    Some(store) => match crate::folders::open(store.session_path()) {
                        Ok(()) => "Opened log folder".into(),
                        Err(error) => format!("Cannot open log folder: {error}"),
                    },
                    None => "Create a saved match before opening its log folder".into(),
                };
            }
            Action::PreviewUp | Action::PreviewDown => {
                self.scroll_preview(matches!(action, Action::PreviewDown));
            }
        }
        false
    }

    pub fn key(&mut self, key: KeyEvent, now: DateTime<FixedOffset>) -> io::Result<bool> {
        if matches!(key.code, KeyCode::Tab | KeyCode::BackTab) {
            return Ok(false);
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && key.code == KeyCode::Char('c') {
            return Ok(self.request_quit());
        }
        let text_editing = !self.help_visible
            && (matches!(self.mode, Mode::Writing)
                || matches!(&self.mode, Mode::Form(form) if form.editing || form.team_searching));
        // Printable bindings belong to the editor, including Space and '?'.
        let literal = matches!(key.code, KeyCode::Char(_))
            && !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
        let team_field = matches!(&self.mode, Mode::Form(form)
            if form.is_team_field() && !form.editing && form.discard_selected.is_none());
        let team_command = if team_field {
            match key.code {
                KeyCode::Enter => Some(Command::Confirm),
                KeyCode::Char(' ') if !text_editing => Some(Command::Confirm),
                KeyCode::Esc => Some(Command::Back),
                KeyCode::Up => Some(Command::Up),
                KeyCode::Down => Some(Command::Down),
                KeyCode::Left => Some(Command::Left),
                KeyCode::Right => Some(Command::Right),
                _ => None,
            }
        } else {
            None
        };
        let command = if team_command.is_some() {
            team_command
        } else if matches!(self.mode, Mode::Idle | Mode::Writing) && key.code == KeyCode::Enter {
            Some(Command::Confirm)
        } else if matches!(self.mode, Mode::Idle | Mode::Writing) && key.code == KeyCode::Esc {
            Some(Command::Back)
        } else if text_editing && literal {
            None
        } else {
            self.keys.command(key)
        };
        if self.help_visible {
            if matches!(
                command,
                Some(Command::Help | Command::Back | Command::Confirm)
            ) {
                self.help_visible = false;
            }
            return Ok(false);
        }
        if command.is_none()
            && key.modifiers.contains(KeyModifiers::SHIFT)
            && matches!(key.code, KeyCode::PageUp | KeyCode::PageDown)
        {
            self.scroll_preview(key.code == KeyCode::PageDown);
            return Ok(false);
        }
        if command == Some(Command::Help) {
            self.help_visible = true;
            return Ok(false);
        }
        // Never execute a popup choice while its controls are hidden by the size guard.
        if !self.controls_visible
            && command == Some(Command::Confirm)
            && (matches!(
                self.mode,
                Mode::Actions { .. } | Mode::Delete { .. } | Mode::ReconnectController { .. }
            ) || matches!(&self.mode, Mode::Form(form)
                    if form.discard_selected.is_some()
                        || form.team_selection.is_some()
                        || form.detected_teams.is_some()
                        || (matches!(form.kind, FormKind::Setup)
                            && form.selected == form.fields.len() + 2)))
        {
            return Ok(false);
        }
        if matches!(self.mode, Mode::Idle) && command == Some(Command::Back) {
            let (message_budget, gc_seconds_remaining) = self.gc_values();
            self.action_capture_after = self.capture_anchor();
            self.mode = Mode::Actions {
                selected: 0,
                timestamp: now,
                message_budget,
                gc_seconds_remaining,
            };
            return Ok(false);
        }
        let mode = std::mem::replace(&mut self.mode, Mode::Idle);
        match mode {
            Mode::Idle => match command {
                Some(Command::Confirm) => {
                    let (message_budget, gc_seconds_remaining) = self.gc_values();
                    self.record(
                        EventKind::Note,
                        now,
                        message_budget,
                        gc_seconds_remaining,
                        self.capture_anchor(),
                    );
                    self.mode = Mode::Writing;
                }
                Some(Command::Up | Command::Left) => self.move_selection(false),
                Some(Command::Down | Command::Right) => self.move_selection(true),
                _ => {}
            },
            Mode::Writing => {
                if matches!(key.code, KeyCode::Enter | KeyCode::Esc)
                    || matches!(command, Some(Command::Confirm | Command::Back))
                {
                    if let Some(session) = &mut self.session {
                        session.submit_note();
                    }
                    self.dirty = true;
                    self.status = "Note saved.".into();
                } else {
                    self.mode = Mode::Writing;
                    if edit_key(&mut self.note_editor, key) {
                        self.sync_note();
                    }
                }
            }
            Mode::Actions {
                mut selected,
                timestamp,
                message_budget,
                gc_seconds_remaining,
            } => {
                let action = match command {
                    Some(Command::Back) => {
                        self.restore_mode();
                        return Ok(false);
                    }
                    Some(Command::Up | Command::Left) => {
                        selected = wrap_selection(selected, ACTIONS.len(), false);
                        None
                    }
                    Some(Command::Down | Command::Right) => {
                        selected = wrap_selection(selected, ACTIONS.len(), true);
                        None
                    }
                    Some(Command::Confirm) => Some(ACTIONS[selected]),
                    _ => None,
                };
                self.mode = Mode::Actions {
                    selected,
                    timestamp,
                    message_budget,
                    gc_seconds_remaining,
                };
                if let Some(action) = action {
                    return Ok(self.perform_action(action, timestamp));
                }
            }
            Mode::ReconnectController { mut selected } => match command {
                Some(Command::Back) => self.restore_mode(),
                Some(Command::Up | Command::Down | Command::Left | Command::Right) => {
                    selected = wrap_selection(selected, 2, true);
                    self.mode = Mode::ReconnectController { selected };
                }
                Some(Command::Confirm) => {
                    self.restore_mode();
                    if selected == 1 {
                        self.reconnect_controller();
                    }
                }
                _ => self.mode = Mode::ReconnectController { selected },
            },
            Mode::Form(mut form) => {
                if let Some(detected) = &form.detected_teams {
                    let apply = command == Some(Command::Confirm) && detected.selected > 0;
                    if let Some(command) = command {
                        form.navigate(command);
                    }
                    if form.detected_teams.is_none() {
                        self.status = if apply {
                            "Detected teams applied to draft as Official match; review before Create match"
                        } else {
                            "Detected team selection cancelled; setup unchanged"
                        }.into();
                    }
                    self.mode = Mode::Form(form);
                } else if form.team_selection.is_some() {
                    if form.team_searching && literal {
                        if let KeyCode::Char(ch) = key.code {
                            form.team_filter.push(ch);
                            form.update_team_filter();
                        }
                    } else if form.team_searching && key.code == KeyCode::Backspace {
                        form.team_filter.pop();
                        form.update_team_filter();
                    } else if key.code == KeyCode::Char('/') && literal && command.is_none() {
                        form.team_searching = true;
                    } else if let Some(command) = command {
                        form.navigate(command);
                    }
                    self.mode = Mode::Form(form);
                } else if let Some(selected) = form.discard_selected {
                    match command {
                        _ if key.code == KeyCode::Esc => form.discard_selected = None,
                        Some(Command::Back) => form.discard_selected = None,
                        Some(Command::Confirm) if selected == 1 => {
                            if matches!(form.kind, FormKind::Setup) {
                                return Ok(true);
                            }
                            self.restore_mode();
                            self.status = "Form cancelled; previous data kept".into();
                            return Ok(false);
                        }
                        Some(Command::Confirm) => form.discard_selected = None,
                        Some(Command::Up | Command::Left | Command::Down | Command::Right) => {
                            form.discard_selected =
                                Some(wrap_selection(form.discard_selected.unwrap_or(0), 2, true));
                        }
                        _ => {}
                    }
                    self.mode = Mode::Form(form);
                } else if form.editing {
                    if matches!(key.code, KeyCode::Enter | KeyCode::Esc)
                        || matches!(command, Some(Command::Confirm | Command::Back))
                    {
                        form.editing = false;
                    } else {
                        edit_key(&mut form.fields[form.selected].editor, key);
                    }
                    self.mode = Mode::Form(form);
                } else if command == Some(Command::Back)
                    || key.code == KeyCode::Esc
                    || (command == Some(Command::Confirm) && form.selected == form.fields.len() + 1)
                {
                    if form.changed() {
                        form.discard_selected = Some(0);
                        self.mode = Mode::Form(form);
                    } else if matches!(form.kind, FormKind::Setup) {
                        return Ok(true);
                    } else {
                        self.restore_mode();
                        self.status = "Form cancelled; previous data kept".into();
                    }
                } else if matches!(form.kind, FormKind::Setup)
                    && command == Some(Command::Confirm)
                    && form.selected == form.fields.len() + 2
                {
                    self.mode = Mode::Form(form);
                    self.offer_detected_teams(self.discovery.detected_teams());
                } else if command == Some(Command::Confirm) && form.selected == form.fields.len() {
                    if let Err(error) = self.apply_form(&form) {
                        self.status = error;
                        self.mode = Mode::Form(form);
                    }
                } else {
                    if let Some(command) = command {
                        form.navigate(command);
                    }
                    self.mode = Mode::Form(form);
                }
            }
            Mode::Delete {
                index,
                mut selected,
            } => match command {
                Some(Command::Back) => self.restore_mode(),
                Some(Command::Confirm) => {
                    if selected == 1 {
                        let was_finished = self.session.as_ref().is_some_and(Session::is_finished);
                        if let Some(session) = &mut self.session {
                            session.delete(index);
                        }
                        self.history.select(Some(index));
                        self.dirty = true;
                        self.status = "Event removed, not erased. Restore it from Actions.".into();
                        self.rebaseline_if_reopened(was_finished);
                    }
                    self.restore_mode();
                }
                _ => {
                    match command {
                        Some(Command::Up | Command::Left | Command::Down | Command::Right) => {
                            selected = wrap_selection(selected, 2, true);
                        }
                        _ => {}
                    }
                    self.mode = Mode::Delete { index, selected };
                }
            },
        }
        Ok(false)
    }

    fn open_correction(&mut self) {
        if let (Some(session), Some(index)) = (&self.session, self.history.selected()) {
            let mut form = Form::entry(index, &session.entries[index]);
            form.match_type = session.entries[index].game.or(session.match_type);
            self.mode = Mode::Form(form);
            self.status =
                "Select a field and confirm to edit; select Save to apply, or Cancel to keep previous data.".into();
        }
    }

    fn rebaseline_if_reopened(&mut self, was_finished: bool) {
        if was_finished
            && self
                .session
                .as_ref()
                .is_some_and(|session| session.match_type.is_some() && !session.is_finished())
        {
            // Packets received while closed advanced the receiver but not the saved score.
            self.pending_controller.clear();
            if let Some(controller) = &mut self.controller {
                controller.rebaseline();
            }
        }
    }

    fn apply_form(&mut self, form: &Form) -> Result<(), String> {
        let was_finished = self.session.as_ref().is_some_and(Session::is_finished);
        let mut controller_stopped = false;
        match form.kind {
            FormKind::Setup => {
                let metadata = form.read_metadata()?;
                let output = form.fields[10].value().trim();
                if output.is_empty() {
                    return Err("Output directory is required".into());
                }
                let session =
                    Session::new_match(metadata, form.match_type.ok_or("Choose a match type")?);
                session.validate()?;
                let store = Store::create(Path::new(output), &session)
                    .map_err(|error| format!("Cannot create log: {error}"))?;
                self.store = Some(store);
                self.session = Some(session);
            }
            FormKind::Metadata => {
                self.flush_controller();
                let metadata = form.read_metadata()?;
                if let Some(session) = &mut self.session {
                    let tracked_changed =
                        session.metadata.tracked_team_number() != metadata.tracked_team_number();
                    if tracked_changed
                        && (!self.pending_controller.is_empty()
                            || self.last_capture_id.is_some_and(|id| id > 0)
                            || !self.pending_capture.is_empty()
                            || session.entries.iter().any(|entry| {
                                entry.has_controller_snapshot() || entry.statistics.is_some()
                            }))
                    {
                        return Err("Cannot change the tracked team after GC history has been captured; create a new match".into());
                    }
                    let teams_changed = tracked_changed
                        || team_number(session.metadata.opponent_name())
                            != team_number(metadata.opponent_name());
                    let mut candidate = session.clone();
                    candidate.metadata = metadata;
                    candidate.validate()?;
                    if teams_changed {
                        controller_stopped = self.controller.take().is_some();
                        self.controller_error = None;
                    } else if let Some(controller) = &mut self.controller {
                        controller.rebaseline();
                    }
                    *session = candidate;
                }
                self.dirty = true;
            }
            FormKind::Entry(index) => {
                if let Some(session) = &mut self.session {
                    let entry = form.read_entry(&session.entries[index])?;
                    let mut candidate = session.clone();
                    if candidate.active_note == Some(index) && entry.kind != EventKind::Note {
                        candidate.submit_note();
                    }
                    candidate.entries[index] = entry;
                    candidate.validate()?;
                    *session = candidate;
                }
                self.dirty = true;
            }
        }
        self.rebaseline_if_reopened(was_finished);
        self.status = if controller_stopped {
            "Changes applied; GC stopped after team change. Enable it again from Actions."
        } else {
            "Changes applied"
        }
        .into();
        self.restore_mode();
        Ok(())
    }

    fn sync_note(&mut self) {
        if let Some(session) = &mut self.session
            && let Some(index) = session.active_note
        {
            session.entries[index].text = self.note_editor.lines().join(" ");
            self.dirty = true;
        }
    }

    pub fn paste(&mut self, text: &str) {
        if self.help_visible {
            return;
        }
        let text: String = text
            .chars()
            .map(|ch| if ch.is_control() { ' ' } else { ch })
            .collect();
        match &mut self.mode {
            Mode::Writing => {
                self.note_editor.insert_str(text);
                self.sync_note();
            }
            Mode::Form(form)
                if form.editing && form.selected < form.fields.len() && !form.is_choice_field() =>
            {
                form.fields[form.selected].editor.insert_str(text);
            }
            _ => {}
        }
    }

    pub fn preview(&self) -> String {
        let mut session = self.session.clone();
        if let Mode::Form(form) = &self.mode {
            match form.kind {
                FormKind::Setup | FormKind::Metadata => {
                    if let Ok(metadata) = form.read_metadata() {
                        if let Some(session) = &mut session {
                            session.metadata = metadata;
                        } else {
                            session = Some(Session::new_match(
                                metadata,
                                form.match_type.unwrap_or(MatchType::Combined),
                            ));
                        }
                    }
                }
                FormKind::Entry(index) => {
                    if let Some(session) = &mut session
                        && let Ok(entry) = form.read_entry(&session.entries[index])
                    {
                        session.entries[index] = entry;
                    }
                }
            }
        }
        session.map_or_else(
            || "Complete the match details to preview the generated log.".into(),
            |session| session.markdown(),
        )
    }
}

fn match_event_kind(match_type: Option<MatchType>, kind: EventKind) -> Result<EventKind, String> {
    match (match_type, kind) {
        (Some(MatchType::GoldenGoal), EventKind::GameStart) => Ok(EventKind::GoldenGoalStart),
        (Some(MatchType::GoldenGoal), EventKind::HalfTime | EventKind::SecondHalfStart) => {
            Err("Golden Goal has no halves. Create a new session for an official match.".into())
        }
        (Some(MatchType::Official), EventKind::GoldenGoalStart) => {
            Err("This is an official match. Create a new Golden Goal session.".into())
        }
        _ => Ok(kind),
    }
}

fn parse_gc_clock(text: &str) -> Result<Option<i16>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    let error = "GC countdown must be signed MM:SS (seconds 00–59), or blank for unavailable";
    let (sign, clock) = text
        .strip_prefix('-')
        .map_or((1, text), |clock| (-1, clock));
    let (minutes, seconds) = clock.split_once(':').ok_or(error)?;
    let minutes = minutes.parse::<u32>().map_err(|_| error)?;
    let seconds = seconds.parse::<u32>().map_err(|_| error)?;
    if seconds >= 60 {
        return Err(error.into());
    }
    let total = minutes
        .checked_mul(60)
        .and_then(|value| value.checked_add(seconds))
        .and_then(|value| i32::try_from(value).ok())
        .and_then(|value| i16::try_from(value * sign).ok())
        .ok_or(error)?;
    Ok(Some(total))
}

fn edit_key(editor: &mut TextArea<'static>, key: KeyEvent) -> bool {
    match key.code {
        KeyCode::Left
        | KeyCode::Right
        | KeyCode::Home
        | KeyCode::End
        | KeyCode::Backspace
        | KeyCode::Delete => editor.input(key),
        KeyCode::Char(ch)
            if !ch.is_control()
                && !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            editor.input(key)
        }
        KeyCode::Char('z') if key.modifiers == KeyModifiers::CONTROL => editor.undo(),
        KeyCode::Char('y') if key.modifiers == KeyModifiers::CONTROL => editor.redo(),
        KeyCode::Char('a' | 'e' | 'k' | 'u' | 'w' | 'r')
            if key.modifiers == KeyModifiers::CONTROL =>
        {
            editor.input(key)
        }
        _ => false,
    }
}

#[cfg(test)]
#[path = "../tests/unit/app.rs"]
mod tests;
