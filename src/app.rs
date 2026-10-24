use std::{io, path::Path};

use chrono::{DateTime, FixedOffset, Local, NaiveDate};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{style::Color, widgets::ListState};
use tui_textarea::{CursorMove, TextArea};

use crate::{
    capture::{CaptureLog, Packet},
    game_controller::{Receiver, Update, format_clock},
    model::{Entry, EventKind, MatchType, Metadata, Session},
    storage::Store,
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

#[derive(Clone, Copy)]
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
}

impl Action {
    pub fn label(self) -> &'static str {
        match self {
            Self::Mark(EventKind::Note) => "New note (keep unfinished draft)",
            Self::Mark(kind) => kind.label(),
            Self::Match => "Edit match details",
            Self::Correct => "Correct selected event",
            Self::Delete => "Remove selected event (reversible)",
            Self::Restore => "Restore removed event",
            Self::Controller => "Toggle GameController (v20 UDP 3838)",
            Self::TrueData => "Request true GC data",
            Self::PreviewUp => "Scroll preview up",
            Self::PreviewDown => "Scroll preview down",
            Self::RetrySave => "Retry saving",
            Self::Undo => "Undo note edit",
            Self::Redo => "Redo note edit",
            Self::Quit => "Save and quit",
        }
    }
}

pub const ACTIONS: &[(char, Action)] = &[
    ('g', Action::Mark(EventKind::GameStart)),
    ('h', Action::Mark(EventKind::HalfTime)),
    ('s', Action::Mark(EventKind::SecondHalfStart)),
    ('1', Action::Mark(EventKind::GoalHome)),
    ('2', Action::Mark(EventKind::GoalAway)),
    ('f', Action::Mark(EventKind::GameEnd)),
    ('x', Action::Mark(EventKind::Custom)),
    ('n', Action::Mark(EventKind::Note)),
    ('t', Action::Mark(EventKind::GoldenGoalStart)),
    ('m', Action::Match),
    ('e', Action::Correct),
    ('d', Action::Delete),
    ('b', Action::Restore),
    ('l', Action::Controller),
    ('v', Action::TrueData),
    ('[', Action::PreviewUp),
    (']', Action::PreviewDown),
    ('w', Action::RetrySave),
    ('u', Action::Undo),
    ('r', Action::Redo),
    ('q', Action::Quit),
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
    Delete(usize),
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
}

pub struct Form {
    pub kind: FormKind,
    pub fields: Vec<Field>,
    pub selected: usize,
    pub match_type: Option<MatchType>,
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
            Field::new("Home team", &metadata.home),
            Field::new("Away team", &metadata.away),
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
            fields.push(Field::new(
                "Match type (←/→ to change)",
                "Golden Goal + official",
            ));
        }
        Self {
            kind,
            fields,
            selected: 0,
            match_type: setup.then_some(MatchType::Combined),
        }
    }

    fn entry(index: usize, entry: &Entry) -> Self {
        Self {
            kind: FormKind::Entry(index),
            fields: vec![
                Field::new("Event type (←/→ to change)", entry.kind.label()),
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
            match_type: None,
        }
    }

    pub fn read_metadata(&self) -> Result<Metadata, String> {
        let metadata = Metadata {
            event: self.fields[0].value().trim().to_owned(),
            home: self.fields[1].value().trim().to_owned(),
            away: self.fields[2].value().trim().to_owned(),
            date: NaiveDate::parse_from_str(self.fields[3].value(), "%Y-%m-%d")
                .map_err(|_| "Date must be YYYY-MM-DD".to_owned())?,
            label: self.fields[4].value().trim().to_owned(),
            robots: std::array::from_fn(|i| self.fields[5 + i].value().trim().to_owned()),
        };
        metadata.validate()?;
        let opponent = if metadata.hulks_are_home() {
            &metadata.away
        } else {
            &metadata.home
        };
        if self.match_type == Some(MatchType::GoldenGoal)
            && !opponent.eq_ignore_ascii_case("Invisibles")
        {
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

    fn is_choice_field(&self) -> bool {
        (matches!(self.kind, FormKind::Entry(_)) && self.selected == 0)
            || (matches!(self.kind, FormKind::Setup) && self.selected == 11)
    }

    fn key(&mut self, key: KeyEvent) {
        let controls = self.fields.len() + 1;
        match key.code {
            KeyCode::Tab | KeyCode::Down | KeyCode::Enter => {
                self.selected = (self.selected + 1) % controls
            }
            KeyCode::BackTab | KeyCode::Up => {
                self.selected = (self.selected + controls - 1) % controls
            }
            KeyCode::Left | KeyCode::Right
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
                let step = if key.code == KeyCode::Left {
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
                    let opponent = if self.fields[2].value().trim().eq_ignore_ascii_case("HULKs") {
                        1
                    } else {
                        2
                    };
                    self.fields[opponent].editor = editor("Invisibles");
                    if self.fields[4].value() == "1st_game" {
                        self.fields[4].editor = editor("Golden_Goal");
                    }
                } else if self.fields[4].value() == "Golden_Goal" {
                    self.fields[4].editor = editor("1st_game");
                }
            }
            KeyCode::Left | KeyCode::Right
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
                let step = if key.code == KeyCode::Left {
                    kinds.len() - 1
                } else {
                    1
                };
                self.fields[0].editor = editor(kinds[(current + step) % kinds.len()].label());
            }
            _ if self.selected < self.fields.len() && !self.is_choice_field() => {
                edit_key(&mut self.fields[self.selected].editor, key);
            }
            _ => {}
        }
    }
}

pub struct App {
    pub session: Option<Session>,
    pub store: Option<Store>,
    pub mode: Mode,
    pub note_editor: TextArea<'static>,
    pub history: ListState,
    pub preview_scroll: Option<u16>,
    pub preview_max: u16,
    pub status: String,
    pub save_error: Option<String>,
    pub dirty: bool,
    pub controller: Option<Receiver>,
    pub controller_error: Option<String>,
    pub pending_controller: Vec<Update>,
    pub capture_log: Option<CaptureLog>,
    pub pending_capture: Vec<Packet>,
    pub last_capture_id: Option<u64>,
    action_capture_after: Option<u64>,
}

impl App {
    pub fn new(path: Option<&Path>) -> io::Result<Self> {
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
                note_editor: editor(""),
                history: ListState::default(),
                preview_scroll: None,
                preview_max: 0,
                status: "Session reopened".into(),
                save_error: None,
                dirty: false,
                controller: None,
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
            event: String::new(),
            home: "HULKs".into(),
            away: String::new(),
            date: Local::now().date_naive(),
            label: "1st_game".into(),
            robots: std::array::from_fn(|_| String::new()),
        };
        let output = std::env::current_dir()?.join("logs");
        Ok(Self {
            session: None,
            store: None,
            mode: Mode::Form(Form::metadata(
                FormKind::Setup,
                &metadata,
                &output.to_string_lossy(),
            )),
            note_editor: editor(""),
            history: ListState::default(),
            preview_scroll: None,
            preview_max: 0,
            status: "Fill match details, then select Create match and press Enter".into(),
            save_error: None,
            dirty: false,
            controller: None,
            controller_error: None,
            pending_controller: Vec::new(),
            capture_log: None,
            pending_capture: Vec::new(),
            last_capture_id: None,
            action_capture_after: None,
        })
    }

    pub fn mode_style(&self) -> (&'static str, Color) {
        if self.save_error.is_some() {
            return ("SAVE FAILED — CHANGES IN MEMORY", Color::LightRed);
        }
        match &self.mode {
            Mode::Idle => ("IDLE — ENTER TO MARK AN EVENT", Color::LightCyan),
            Mode::Writing => ("WRITING — TIMESTAMP CAPTURED", Color::LightGreen),
            Mode::Actions { .. } => ("ACTION MENU — TIME CAPTURED", Color::LightMagenta),
            Mode::Form(form) => match form.kind {
                FormKind::Setup => ("MATCH SETUP — NOT CREATED YET", Color::LightBlue),
                FormKind::Metadata => ("EDIT MATCH — SELECT APPLY", Color::LightYellow),
                FormKind::Entry(_) => ("CORRECTION — SELECT APPLY", Color::LightYellow),
            },
            Mode::Delete(_) => ("REMOVE EVENT — CAN BE RESTORED", Color::LightRed),
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
                    "Raw capture save failed: {error}. Tab, W retries; quitting is blocked."
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
                        "Save failed: {error}. Tab, W retries; quitting is blocked."
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
                    Some(packet.hulks_message_budget),
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
            self.status = "Waiting for GC opponent change. Stop listening to start the official game manually.".into();
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
                "GC owns goals while listening. Tab, L stops listening for manual goals.".into();
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
        if matches!(&self.mode, Mode::Form(form) if !matches!(form.kind, FormKind::Setup)) {
            self.status = "Select Apply or cancel the form with Esc before quitting".into();
            return false;
        }
        self.poll_controller();
        self.flush_controller();
        self.persist();
        self.save_error.is_none()
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
                        self.status = "Event already removed; Tab, B restores it".into();
                    } else {
                        self.mode = Mode::Delete(index);
                    }
                }
            }
            Action::Restore => {
                if let (Some(session), Some(index)) = (&mut self.session, self.history.selected()) {
                    session.restore(index);
                    self.dirty = true;
                    self.status =
                        "Event restored at its original timestamp; score recalculated".into();
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
                    match Receiver::bind() {
                        Ok(mut receiver) => {
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
                        "Start GC listening with Tab, L first; wait for a controller, then Tab, V"
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
            Action::PreviewUp | Action::PreviewDown => {
                self.scroll_preview(matches!(action, Action::PreviewDown));
            }
        }
        false
    }

    pub fn key(&mut self, key: KeyEvent, now: DateTime<FixedOffset>) -> io::Result<bool> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && key.code == KeyCode::Char('c') {
            return Ok(self.request_quit());
        }
        if key.modifiers.contains(KeyModifiers::SHIFT)
            && matches!(key.code, KeyCode::PageUp | KeyCode::PageDown)
        {
            self.scroll_preview(key.code == KeyCode::PageDown);
            return Ok(false);
        }
        if matches!(self.mode, Mode::Idle | Mode::Writing) {
            let action = match key.code {
                KeyCode::Tab | KeyCode::F(2) => {
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
                KeyCode::F(3) => Some(Action::Match),
                KeyCode::F(4) => Some(Action::Correct),
                KeyCode::F(5) => Some(Action::Mark(EventKind::GoalHome)),
                KeyCode::F(6) => Some(Action::Mark(EventKind::GoalAway)),
                KeyCode::Char('g') if ctrl => Some(Action::Mark(EventKind::GameStart)),
                KeyCode::Char('s') if ctrl => Some(Action::RetrySave),
                _ => None,
            };
            if let Some(action) = action {
                return Ok(self.perform_action(action, now));
            }
        }
        let mode = std::mem::replace(&mut self.mode, Mode::Idle);
        match mode {
            Mode::Idle => match key.code {
                KeyCode::Enter => {
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
                KeyCode::Up => self.move_selection(false),
                KeyCode::Down => self.move_selection(true),
                KeyCode::Char('e') => self.open_correction(),
                KeyCode::Delete | KeyCode::Char('d') => {
                    self.perform_action(Action::Delete, now);
                }
                _ => {}
            },
            Mode::Writing => {
                if matches!(key.code, KeyCode::Enter | KeyCode::Esc) {
                    if let Some(session) = &mut self.session {
                        session.submit_note();
                    }
                    self.dirty = true;
                    self.status = "Note saved. Enter marks the next event.".into();
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
                let action = match key.code {
                    KeyCode::Esc | KeyCode::Tab => {
                        self.restore_mode();
                        return Ok(false);
                    }
                    KeyCode::Up => {
                        selected = selected.saturating_sub(1);
                        None
                    }
                    KeyCode::Down => {
                        selected = (selected + 1).min(ACTIONS.len() - 1);
                        None
                    }
                    KeyCode::Enter => Some(ACTIONS[selected].1),
                    KeyCode::Char(ch)
                        if !key
                            .modifiers
                            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                    {
                        ACTIONS
                            .iter()
                            .position(|(shortcut, _)| *shortcut == ch.to_ascii_lowercase())
                            .map(|index| {
                                selected = index;
                                ACTIONS[index].1
                            })
                    }
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
            Mode::Form(mut form) => {
                if key.code == KeyCode::Esc {
                    if matches!(form.kind, FormKind::Setup) {
                        return Ok(true);
                    }
                    self.restore_mode();
                    self.status = "Form cancelled; previous data kept".into();
                } else if (ctrl && key.code == KeyCode::Char('s'))
                    || (key.code == KeyCode::Enter && form.selected == form.fields.len())
                {
                    if let Err(error) = self.apply_form(&form) {
                        self.status = error;
                        self.mode = Mode::Form(form);
                    }
                } else {
                    form.key(key);
                    self.mode = Mode::Form(form);
                }
            }
            Mode::Delete(index) => match key.code {
                KeyCode::Char('y') => {
                    let was_finished = self.session.as_ref().is_some_and(Session::is_finished);
                    if let Some(session) = &mut self.session {
                        session.delete(index);
                    }
                    self.history.select(Some(index));
                    self.dirty = true;
                    self.status = "Event removed, not erased. Tab, B restores it.".into();
                    self.rebaseline_if_reopened(was_finished);
                    self.restore_mode();
                }
                KeyCode::Esc | KeyCode::Char('n') => self.restore_mode(),
                _ => self.mode = Mode::Delete(index),
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
                "Edit description, wall-clock timestamp, GC countdown or event type. Select Apply; Esc cancels.".into();
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
        match form.kind {
            FormKind::Setup => {
                let metadata = form.read_metadata()?;
                let output = form.fields[10].value().trim();
                if output.is_empty() {
                    return Err("Output directory is required".into());
                }
                let session =
                    Session::new_match(metadata, form.match_type.ok_or("Choose a match type")?);
                let store = Store::create(Path::new(output), &session)
                    .map_err(|error| format!("Cannot create log: {error}"))?;
                self.store = Some(store);
                self.session = Some(session);
            }
            FormKind::Metadata => {
                let metadata = form.read_metadata()?;
                if let Some(session) = &mut self.session {
                    session.metadata = metadata;
                    if let Some(controller) = &mut self.controller {
                        controller.rebaseline();
                    }
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
        self.status = "Changes applied".into();
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
        let text: String = text
            .chars()
            .map(|ch| if ch.is_control() { ' ' } else { ch })
            .collect();
        match &mut self.mode {
            Mode::Writing => {
                self.note_editor.insert_str(text);
                self.sync_note();
            }
            Mode::Form(form) if form.selected < form.fields.len() && !form.is_choice_field() => {
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
