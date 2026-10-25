use std::{
    fmt::Write as _,
    io,
    process::Command as ProcessCommand,
    time::{Duration, Instant},
};

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::{
    Frame, Terminal,
    backend::Backend,
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, List, ListItem, ListState, Padding, Paragraph, Wrap},
};
use tui_textarea::TextArea;

use crate::{
    game_controller::format_clock,
    model::{Entry, EventKind, MatchType, PostgameNote, Session},
    navigation::{Command, Keybindings, wrap_selection},
    presentation::{ACCENT, ERROR, FOCUS, MUTED, SUCCESS, popup, solid_panel},
    settings::Settings,
    storage::Store,
};

const FIELD_NAMES: [&str; 4] = [
    "Issue title",
    "Robots / player numbers",
    "Volunteer / owner",
    "Discussion / decision",
];

struct Jump {
    query: TextArea<'static>,
    selected: usize,
}

struct Edit {
    fields: [TextArea<'static>; 4],
    original: [String; 4],
    selected: usize,
    typing: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Menu {
    Actions,
    FinishWithTabs,
    FinishWithoutTabs,
    DiscardDetails,
}

pub struct Postgame {
    session: Session,
    entries: Vec<usize>,
    selected: usize,
    scroll: u16,
    edit: Option<Edit>,
    jump: Option<Jump>,
    menu: Option<Menu>,
    menu_selected: usize,
    help: bool,
    keys: Keybindings,
    dirty: bool,
    error: Option<String>,
    layout_ready: bool,
    replay: Option<crate::replay::Replay>,
}

impl Postgame {
    fn new_with_settings(session: Session, settings: &Settings) -> Self {
        let entries: Vec<usize> = session
            .entries
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| {
                let diagnostic = entry.has_controller_snapshot()
                    && entry.kind == EventKind::Custom
                    && entry
                        .statistics
                        .as_ref()
                        .is_none_or(|stats| stats.penalties.is_empty());
                (!entry.undone && !diagnostic).then_some(index)
            })
            .collect();
        let selected = session
            .postgame_position
            .and_then(|bookmark| {
                entries
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, index)| index.abs_diff(bookmark))
                    .map(|(position, _)| position)
            })
            .unwrap_or(0);
        Self {
            session,
            entries,
            selected,
            scroll: 0,
            edit: None,
            jump: None,
            menu: None,
            menu_selected: 0,
            help: false,
            keys: settings.keys.clone(),
            dirty: false,
            error: None,
            layout_ready: true,
            replay: None,
        }
    }

    fn new_replay_with_settings(
        session: Session,
        settings: &Settings,
        store: &Store,
    ) -> io::Result<Self> {
        let replay = crate::replay::Replay::load(store, &session)?;
        let mut app = Self::new_with_settings(session, settings);
        app.entries = replay.note_entries();
        app.selected = replay
            .current_entry()
            .and_then(|index| app.entries.iter().position(|&entry| entry == index))
            .unwrap_or(0);
        app.replay = Some(replay);
        Ok(app)
    }

    fn current_index(&self) -> Option<usize> {
        match &self.replay {
            Some(replay) => replay.current_entry(),
            None => self.entries.get(self.selected).copied(),
        }
    }

    fn current(&self) -> Option<&Entry> {
        self.current_index()
            .and_then(|index| self.session.entries.get(index))
    }

    fn sync_replay(&mut self, previous: Option<usize>, store: &Store) {
        let Some(replay) = &self.replay else {
            return;
        };
        let (current, time, paused) = (replay.current_entry(), replay.time, replay.paused);
        if current != previous {
            if let Some(index) = current {
                if let Some(position) = self.entries.iter().position(|&entry| entry == index) {
                    self.selected = position;
                }
                if self.session.postgame_position != Some(index) {
                    self.session.postgame_position = Some(index);
                    self.dirty = true;
                }
            }
        }
        if (paused || current != previous) && self.session.replay_position != Some(time) {
            self.session.replay_position = Some(time);
            self.dirty = true;
        }
        self.save(store);
    }

    fn tick_replay(&mut self, elapsed: Duration, store: &Store) {
        if self.replay.is_none() {
            return;
        }
        if self.menu.is_some()
            || self.edit.is_some()
            || self.jump.is_some()
            || self.help
            || !self.layout_ready
            || (self.dirty && self.error.is_some())
        {
            self.replay.as_mut().unwrap().paused = true;
            return;
        }
        let previous = self.current_index();
        self.replay
            .as_mut()
            .unwrap()
            .advance(elapsed, Instant::now(), &self.session);
        self.sync_replay(previous, store);
    }

    fn save(&mut self, store: &Store) -> bool {
        if !self.dirty {
            return true;
        }
        match store.save(&self.session) {
            Ok(()) => {
                self.dirty = false;
                self.error = None;
                true
            }
            Err(error) => {
                self.error = Some(format!(
                    "Save failed: {error}. Finish retries; exit is blocked."
                ));
                false
            }
        }
    }

    fn links(&self) -> Vec<String> {
        self.session
            .entries
            .iter()
            .enumerate()
            .filter(|(index, entry)| {
                !entry.undone
                    && (self.replay.is_some() || self.entries.binary_search(index).is_ok())
            })
            .filter_map(|(index, entry)| {
                entry
                    .postgame
                    .as_ref()
                    .filter(|note| note.actionable)
                    .map(|note| issue_url(&self.session, index, entry, note))
            })
            .collect()
    }

    fn select(&mut self, selected: usize, store: &Store) {
        let Some(&index) = self.entries.get(selected) else {
            return;
        };
        if self.replay.is_some() {
            let previous = self.current_index();
            self.replay.as_mut().unwrap().seek_entry(index);
            self.sync_replay(previous, store);
            return;
        }
        self.selected = selected;
        self.scroll = 0;
        if self.session.postgame_position != Some(index) {
            self.session.postgame_position = Some(index);
            self.dirty = true;
        }
        self.save(store);
    }

    fn matching_notes(&self, query: &str) -> Vec<usize> {
        let query = query.trim().to_lowercase();
        let number = query.parse::<usize>().ok();
        self.entries
            .iter()
            .enumerate()
            .filter_map(|(position, &index)| {
                let entry = &self.session.entries[index];
                let matches = if let Some(number) = number {
                    position + 1 == number
                } else {
                    entry.text.to_lowercase().contains(&query)
                        || entry.kind.label().to_lowercase().contains(&query)
                };
                matches.then_some(position)
            })
            .collect()
    }

    fn request_exit(&mut self, store: &Store, links: Vec<String>) -> Option<Vec<String>> {
        self.save(store).then_some(links)
    }

    fn cancel_details(&mut self) {
        if self.edit.as_ref().is_some_and(|edit| {
            edit.fields
                .iter()
                .zip(&edit.original)
                .any(|(field, original)| field.lines().join("\n") != *original)
        }) {
            self.open_menu(Menu::DiscardDetails);
        } else {
            self.edit = None;
            self.error = None;
        }
    }

    fn open_menu(&mut self, menu: Menu) {
        if let Some(replay) = &mut self.replay {
            replay.paused = true;
        }
        self.menu = Some(menu);
        self.menu_selected = 0;
    }

    fn menu_items(&self, menu: Menu) -> Vec<&'static str> {
        match menu {
            Menu::Actions => vec![
                if self.current().is_some_and(|entry| {
                    entry.postgame.as_ref().is_some_and(|note| note.actionable)
                }) {
                    "Unmark task"
                } else {
                    "Mark task"
                },
                "Edit details",
                "Jump to note",
                "Open log folder",
                "Back to presentation",
                "Finish + GitHub tabs",
                "Finish meeting",
            ],
            Menu::FinishWithTabs => vec!["Cancel", "Finish and open GitHub tabs"],
            Menu::FinishWithoutTabs => vec!["Cancel", "Finish meeting"],
            Menu::DiscardDetails => vec!["Cancel", "Discard changes"],
        }
    }

    fn activate_menu(&mut self, menu: Menu, store: &Store) -> Option<Vec<String>> {
        self.menu = None;
        match menu {
            Menu::DiscardDetails => {
                if self.menu_selected == 1 {
                    self.edit = None;
                    self.error = None;
                }
            }
            Menu::FinishWithTabs | Menu::FinishWithoutTabs => {
                if self.menu_selected == 1 {
                    let links = if menu == Menu::FinishWithTabs {
                        self.links()
                    } else {
                        Vec::new()
                    };
                    return self.request_exit(store, links);
                }
            }
            Menu::Actions => match self.menu_selected {
                0 => {
                    if let Some(index) = self.current_index() {
                        let note = self.session.entries[index]
                            .postgame
                            .get_or_insert_with(PostgameNote::default);
                        note.actionable = !note.actionable;
                        self.dirty = true;
                        self.save(store);
                    }
                }
                1 => {
                    if let Some(entry) = self.current() {
                        let note = entry.postgame.clone().unwrap_or_default();
                        let original = [
                            issue_title(entry, &note),
                            note.robots,
                            note.owner,
                            note.discussion,
                        ];
                        let fields = original.each_ref().map(|value| {
                            TextArea::new(value.split('\n').map(str::to_owned).collect())
                        });
                        self.edit = Some(Edit {
                            fields,
                            original,
                            selected: 0,
                            typing: false,
                        });
                    }
                }
                5 => self.open_menu(Menu::FinishWithTabs),
                6 => self.open_menu(Menu::FinishWithoutTabs),
                2 => {
                    let mut query = TextArea::default();
                    query.set_style(Style::default().fg(FOCUS).bg(Color::Black));
                    query.set_cursor_style(Style::default().fg(Color::Black).bg(ACCENT));
                    self.jump = Some(Jump {
                        query,
                        selected: self.selected,
                    });
                }
                3 => {
                    if let Err(error) = crate::folders::open(store.session_path()) {
                        self.error = Some(format!("Cannot open log folder: {error}"));
                    }
                }
                _ => {}
            },
        }
        None
    }

    fn key(&mut self, key: KeyEvent, store: &Store) -> Option<Vec<String>> {
        if key.kind == KeyEventKind::Release || matches!(key.code, KeyCode::Tab | KeyCode::BackTab)
        {
            return None;
        }
        if matches!(key.code, KeyCode::Char('c' | 'C'))
            && key.modifiers.contains(KeyModifiers::CONTROL)
        {
            if let Some(edit) = &mut self.edit {
                edit.typing = false;
                self.error =
                    Some("Apply or cancel the details draft before ending the meeting.".into());
            } else {
                self.help = false;
                self.jump = None;
                self.open_menu(Menu::FinishWithoutTabs);
            }
            return None;
        }
        if let Some(edit) = &mut self.edit
            && edit.typing
        {
            if key.code == KeyCode::Esc {
                edit.typing = false;
            } else {
                edit.fields[edit.selected].input(key);
            }
            return None;
        }
        if let Some(jump) = &self.jump {
            let matches = self.matching_notes(&jump.query.lines().join(" "));
            let jump = self.jump.as_mut().unwrap();
            match key.code {
                KeyCode::Esc => self.jump = None,
                KeyCode::Enter => {
                    if self.layout_ready
                        && let Some(&selected) = matches.get(jump.selected)
                    {
                        self.jump = None;
                        self.select(selected, store);
                    }
                }
                KeyCode::Up => {
                    jump.selected = wrap_selection(jump.selected, matches.len(), false);
                }
                KeyCode::Down => {
                    jump.selected = wrap_selection(jump.selected, matches.len(), true);
                }
                _ => {
                    if jump.query.input(key) {
                        jump.selected = 0;
                    }
                }
            }
            return None;
        }
        let command = self.keys.command(key);
        if self.help {
            if matches!(command, Some(Command::Back | Command::Help)) {
                self.help = false;
            }
            return None;
        }
        if command == Some(Command::Help) {
            if let Some(replay) = &mut self.replay {
                replay.paused = true;
            }
            self.help = true;
            return None;
        }
        if !self.layout_ready && command == Some(Command::Confirm) {
            return None;
        }
        if let Some(menu) = self.menu {
            match command {
                Some(Command::Up | Command::Left) => {
                    self.menu_selected =
                        wrap_selection(self.menu_selected, self.menu_items(menu).len(), false);
                }
                Some(Command::Down | Command::Right) => {
                    self.menu_selected =
                        wrap_selection(self.menu_selected, self.menu_items(menu).len(), true);
                }
                Some(Command::Back) => {
                    self.menu = None;
                }
                Some(Command::Confirm) => return self.activate_menu(menu, store),
                _ => {}
            }
            return None;
        }
        if let Some(edit) = &mut self.edit {
            match command {
                Some(Command::Up | Command::Left) => {
                    edit.selected = wrap_selection(edit.selected, FIELD_NAMES.len() + 2, false);
                }
                Some(Command::Down | Command::Right) => {
                    edit.selected = wrap_selection(edit.selected, FIELD_NAMES.len() + 2, true);
                }
                Some(Command::Confirm) if edit.selected < FIELD_NAMES.len() => edit.typing = true,
                Some(Command::Confirm) if edit.selected == FIELD_NAMES.len() => {
                    let values = edit.fields.each_ref().map(|field| field.lines().join("\n"));
                    let [title, robots, owner, discussion] = values;
                    let Some(index) = self.current_index() else {
                        return None;
                    };
                    let entry = &mut self.session.entries[index];
                    let note = entry.postgame.get_or_insert_with(PostgameNote::default);
                    note.title = title;
                    note.robots = robots;
                    note.owner = owner;
                    note.discussion = discussion;
                    self.edit = None;
                    self.dirty = true;
                    self.save(store);
                }
                Some(Command::Back | Command::Confirm) => self.cancel_details(),
                _ => {}
            }
            return None;
        }
        if self.replay.is_some() {
            let previous = self.current_index();
            if self.replay.as_mut().unwrap().key(key) {
                self.sync_replay(previous, store);
                return None;
            }
            match key.code {
                KeyCode::Esc => self.open_menu(Menu::Actions),
                KeyCode::Char('q' | 'Q') => self.open_menu(Menu::FinishWithoutTabs),
                KeyCode::Char('e' | 'E' | 'a' | 'A') => {
                    self.replay.as_mut().unwrap().paused = true;
                    self.menu_selected = usize::from(matches!(key.code, KeyCode::Char('e' | 'E')));
                    return self.activate_menu(Menu::Actions, store);
                }
                _ => match command {
                    Some(Command::Up | Command::Down | Command::Left | Command::Right) => {
                        let code = match command.unwrap() {
                            Command::Up => KeyCode::Up,
                            Command::Down => KeyCode::Down,
                            Command::Left => KeyCode::Left,
                            _ => KeyCode::Right,
                        };
                        self.replay
                            .as_mut()
                            .unwrap()
                            .key(KeyEvent::new(code, KeyModifiers::NONE));
                        self.sync_replay(previous, store);
                    }
                    Some(Command::Confirm | Command::Back) => self.open_menu(Menu::Actions),
                    _ => {}
                },
            }
            return None;
        }
        match command {
            Some(Command::Down | Command::Right) => {
                self.select(
                    (self.selected + 1).min(self.entries.len().saturating_sub(1)),
                    store,
                );
            }
            Some(Command::Up | Command::Left) => {
                self.select(self.selected.saturating_sub(1), store);
            }
            Some(Command::Confirm | Command::Back) => self.open_menu(Menu::Actions),
            _ if key.code == KeyCode::PageDown => self.scroll = self.scroll.saturating_add(5),
            _ if key.code == KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(5),
            _ => {}
        }
        None
    }

    fn paste(&mut self, text: &str) {
        if let Some(jump) = &mut self.jump {
            jump.query.insert_str(
                text.chars()
                    .map(|ch| {
                        if ch.is_control() || matches!(ch, '\u{2028}' | '\u{2029}') {
                            ' '
                        } else {
                            ch
                        }
                    })
                    .collect::<String>(),
            );
            jump.selected = 0;
        } else if let Some(edit) = self.edit.as_mut().filter(|edit| edit.typing) {
            let text: String = text
                .chars()
                .map(|ch| {
                    if (ch.is_control() && ch != '\n') || matches!(ch, '\u{2028}' | '\u{2029}') {
                        ' '
                    } else {
                        ch
                    }
                })
                .collect();
            edit.fields[edit.selected].insert_str(text);
        }
    }

    fn draw(&mut self, frame: &mut Frame) {
        frame.render_widget(
            Block::default().style(Style::default().bg(Color::Black)),
            frame.area(),
        );
        let minimum_height = if self.replay.is_some() { 24 } else { 20 };
        self.layout_ready = frame.area().width >= 60 && frame.area().height >= minimum_height;
        let error = self.error.as_ref();
        if !self.layout_ready {
            frame.render_widget(
                Paragraph::new(format!(
                    "{}\nResize to at least 60 × {minimum_height}. Confirmations disabled.\n{}: back; ending a meeting requires a visible confirmation.",
                    error.map(String::as_str).unwrap_or_default(), self.keys.hint(Command::Back)
                ))
                    .wrap(Wrap { trim: false })
                    .style(Style::default().fg(if error.is_some() { ERROR } else { FOCUS })),
                frame.area(),
            );
            return;
        }
        let roomy = frame.area().width >= 80 && frame.area().height >= 28;
        let padding = if roomy {
            Padding::new(2, 2, 1, 1)
        } else {
            Padding::ZERO
        };
        let plain_replay = self.replay.is_some()
            && self.edit.is_none()
            && self.jump.is_none()
            && self.menu.is_none()
            && !self.help;
        let footer_height = u16::from(!plain_replay);
        let rows = Layout::vertical([
            Constraint::Length(if self.replay.is_some() { 1 } else { 2 }),
            Constraint::Min(8),
            Constraint::Length(footer_height),
        ])
        .split(frame.area());
        let marked = self
            .entries
            .iter()
            .filter(|&&index| {
                self.session.entries[index]
                    .postgame
                    .as_ref()
                    .is_some_and(|note| note.actionable)
            })
            .count();
        if self.replay.is_some() {
            frame.render_widget(
                Paragraph::new(error.map(String::as_str).unwrap_or("REPLAY"))
                    .style(Style::default().fg(if error.is_some() { ERROR } else { SUCCESS })),
                rows[0],
            );
        } else {
            let (home, away) =
                self.session
                    .teams_for(if self.session.match_type == Some(MatchType::GoldenGoal) {
                        MatchType::GoldenGoal
                    } else {
                        MatchType::Official
                    });
            frame.render_widget(
                Paragraph::new(vec![
                    Line::styled(
                        format!(
                            "POSTGAME — {home} vs {away} — {}",
                            self.session.metadata.label,
                        ),
                        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                    ),
                    if let Some(error) = error {
                        Line::styled(error.as_str(), Style::default().fg(ERROR))
                    } else {
                        Line::styled(
                            format!(
                                "Note {}/{} · {marked} actions",
                                if self.entries.is_empty() {
                                    0
                                } else {
                                    self.selected + 1
                                },
                                self.entries.len(),
                            ),
                            Style::default().fg(MUTED),
                        )
                    },
                ]),
                rows[0],
            );
        }
        if let Some(jump) = &self.jump {
            let areas =
                Layout::vertical([Constraint::Length(3), Constraint::Min(3)]).split(rows[1]);
            let query_area = solid_panel(
                frame,
                areas[0],
                Block::bordered().title("Jump to note · text or agenda number"),
                ACCENT,
            );
            frame.render_widget(&jump.query, query_area);
            let matches = self.matching_notes(&jump.query.lines().join(" "));
            let inner = solid_panel(
                frame,
                areas[1],
                Block::bordered().title("Matching notes"),
                MUTED,
            );
            if matches.is_empty() {
                frame.render_widget(Paragraph::new("No matching notes."), inner);
            } else {
                let items = matches.iter().map(|&position| {
                    let entry = &self.session.entries[self.entries[position]];
                    ListItem::new(format!(
                        "{}. {}",
                        position + 1,
                        entry
                            .text
                            .lines()
                            .next()
                            .filter(|text| !text.is_empty())
                            .unwrap_or(entry.kind.label())
                    ))
                });
                frame.render_stateful_widget(
                    List::new(items)
                        .highlight_symbol("▶ ")
                        .highlight_style(Style::default().fg(Color::Black).bg(ACCENT)),
                    inner,
                    &mut ListState::default().with_selected(Some(jump.selected)),
                );
            }
        } else if let Some(edit) = &mut self.edit {
            let areas = Layout::horizontal([
                Constraint::Length(if roomy { 33 } else { 29 }),
                Constraint::Min(20),
            ])
            .split(rows[1]);
            let choices: Vec<_> = FIELD_NAMES
                .iter()
                .copied()
                .chain(["Apply and save", "Cancel changes"])
                .collect();
            let inner = solid_panel(
                frame,
                areas[0],
                Block::bordered().padding(padding).title(if edit.typing {
                    "Details · typing"
                } else {
                    "Details · select field"
                }),
                ACCENT,
            );
            frame.render_stateful_widget(
                List::new(choices)
                    .style(Style::default().fg(MUTED))
                    .highlight_symbol("▶ ")
                    .highlight_style(
                        Style::default()
                            .fg(Color::Black)
                            .bg(ACCENT)
                            .add_modifier(Modifier::BOLD),
                    ),
                inner,
                &mut ListState::default().with_selected(Some(edit.selected)),
            );
            if edit.selected < FIELD_NAMES.len() {
                let field = &mut edit.fields[edit.selected];
                field.set_style(Style::default().fg(FOCUS).bg(Color::Black));
                field.set_cursor_style(Style::default().fg(Color::Black).bg(ACCENT));
                field.remove_block();
                let inner = solid_panel(
                    frame,
                    areas[1],
                    Block::bordered()
                        .padding(padding)
                        .title(FIELD_NAMES[edit.selected]),
                    if edit.typing { ACCENT } else { MUTED },
                );
                frame.render_widget(&*field, inner);
            } else {
                let inner = solid_panel(
                    frame,
                    areas[1],
                    Block::bordered().padding(padding).title("Details · draft"),
                    MUTED,
                );
                frame.render_widget(
                    Paragraph::new(if edit.selected == FIELD_NAMES.len() {
                        "Apply and save"
                    } else {
                        "Discard draft"
                    })
                    .wrap(Wrap { trim: false }),
                    inner,
                );
            }
        } else if let Some(replay) = &mut self.replay {
            replay.draw(frame, rows[1], &self.session, Instant::now());
        } else {
            let columns =
                Layout::horizontal([Constraint::Percentage(65), Constraint::Percentage(35)])
                    .split(rows[1]);
            let neighbor_height = if roomy { 5 } else { 3 };
            let focus = Layout::vertical([
                Constraint::Length(neighbor_height),
                Constraint::Min(4),
                Constraint::Length(neighbor_height),
            ])
            .split(columns[0]);
            for (title, position, empty, area) in [
                (
                    "Previous",
                    self.selected.checked_sub(1),
                    "Start of agenda",
                    focus[0],
                ),
                ("Next", Some(self.selected + 1), "End of agenda", focus[2]),
            ] {
                let index = position.and_then(|position| self.entries.get(position));
                let entry = index.map(|&index| &self.session.entries[index]);
                let title = index
                    .map(|&index| format!("{title} · {}", presentation_time(&self.session, index)))
                    .unwrap_or_else(|| title.to_owned());
                let preview = entry
                    .map(|entry| {
                        entry
                            .display_text()
                            .lines()
                            .find(|line| !line.trim().is_empty())
                            .unwrap_or(entry.kind.label())
                            .to_owned()
                    })
                    .unwrap_or_else(|| empty.to_owned());
                let inner = solid_panel(
                    frame,
                    area,
                    Block::bordered().padding(padding).title(title),
                    MUTED,
                );
                frame.render_widget(
                    Paragraph::new(preview).style(Style::default().fg(MUTED).bg(Color::Black)),
                    inner,
                );
            }
            let text = self
                .current()
                .map(|entry| {
                    if entry.text.trim().is_empty() {
                        entry.kind.label().to_owned()
                    } else {
                        entry.display_text()
                    }
                })
                .unwrap_or_else(|| "No active notes or events to present.".into());
            let marked = self
                .current()
                .and_then(|entry| entry.postgame.as_ref())
                .is_some_and(|note| note.actionable);
            let block = Block::bordered()
                .padding(padding)
                .title(Line::from(vec![
                    Span::raw("Current note"),
                    Span::styled(
                        if marked { " · ACTION ITEM" } else { "" },
                        Style::default()
                            .fg(Color::Black)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]))
                .title_bottom(Line::styled(
                    self.entries
                        .get(self.selected)
                        .map(|&index| presentation_time(&self.session, index))
                        .unwrap_or_default(),
                    Style::default()
                        .fg(Color::Black)
                        .remove_modifier(Modifier::BOLD),
                ));
            let inner = solid_panel(frame, focus[1], block, FOCUS);
            let paragraph = Paragraph::new(text)
                .style(
                    Style::default()
                        .fg(FOCUS)
                        .bg(Color::Black)
                        .add_modifier(Modifier::BOLD),
                )
                .wrap(Wrap { trim: false });
            let max_scroll = paragraph
                .line_count(inner.width)
                .saturating_sub(usize::from(inner.height));
            self.scroll = self
                .scroll
                .min(u16::try_from(max_scroll).unwrap_or(u16::MAX));
            frame.render_widget(paragraph.scroll((self.scroll, 0)), inner);
            let items: Vec<_> = self
                .entries
                .iter()
                .enumerate()
                .map(|(position, &index)| {
                    let entry = &self.session.entries[index];
                    let marked = entry.postgame.as_ref().is_some_and(|note| note.actionable);
                    let time = entry
                        .gc_seconds_remaining
                        .map(format_clock)
                        .unwrap_or_else(|| entry.timestamp.format("%H:%M:%S").to_string());
                    ListItem::new(Line::from(vec![
                        Span::styled(if marked { "*" } else { " " }, Style::default().fg(SUCCESS)),
                        Span::raw(format!(
                            " {}. [{time}] {}",
                            position + 1,
                            entry
                                .display_text()
                                .lines()
                                .next()
                                .filter(|text| !text.is_empty())
                                .unwrap_or(entry.kind.label())
                        )),
                    ]))
                })
                .collect();
            let mut state = ListState::default()
                .with_selected((!self.entries.is_empty()).then_some(self.selected));
            let inner = solid_panel(
                frame,
                columns[1],
                Block::bordered().padding(padding).title("Full agenda"),
                MUTED,
            );
            frame.render_stateful_widget(
                List::new(items)
                    .style(Style::default().fg(MUTED))
                    .scroll_padding(1)
                    .highlight_symbol("▶ ")
                    .highlight_style(Style::default().fg(FOCUS).add_modifier(Modifier::BOLD)),
                inner,
                &mut state,
            );
        }
        if self.help {
            let text = if self.replay.is_some() {
                format!(
                    "Space: play/pause · ↑↓: notes · ←→: seek\nHome/End: start/end · PgUp/PgDn: scroll note\n+/-: speed · R: auto · B: balls · H: opponents\n1–5: own robot · 6–9/0: opponent\nA: actionable · E: details · Q: finish\nEsc / {}: meeting menu\nMenus: {}/{} move · {} select\nDetails: Esc finishes editing.",
                    self.keys.hint(Command::Back),
                    self.keys.hint(Command::Up),
                    self.keys.hint(Command::Down),
                    self.keys.hint(Command::Confirm)
                )
            } else {
                format!(
                    "{}/{}: previous/next\n{}/{}: previous/next\n{}: actions/select\n{}: menu/back\n{}: help\nPgUp/PgDn: scroll\nDetails: Esc finishes editing.\nJump to note: text or agenda number.",
                    self.keys.hint(Command::Up),
                    self.keys.hint(Command::Down),
                    self.keys.hint(Command::Left),
                    self.keys.hint(Command::Right),
                    self.keys.hint(Command::Confirm),
                    self.keys.hint(Command::Back),
                    self.keys.hint(Command::Help),
                )
            };
            let inner = solid_panel(
                frame,
                rows[1],
                Block::bordered().padding(padding).title("Postgame help"),
                ACCENT,
            );
            frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), inner);
        } else if let Some(menu) = self.menu {
            let (title, text) = match menu {
                Menu::Actions => ("Actions · current note", "".into()),
                Menu::DiscardDetails => (
                    "Discard unsaved details?",
                    "Your details draft has changes. Discard them and return to presentation?\nCancel keeps the draft.".into(),
                ),
                Menu::FinishWithTabs => (
                    "Finish meeting?",
                    format!("Open {marked} prefilled GitHub tabs in Safari?\nNothing is submitted."),
                ),
                Menu::FinishWithoutTabs => (
                    "Finish meeting?",
                    "Save and finish without opening browser tabs?".into(),
                ),
            };
            let destructive = matches!(menu, Menu::DiscardDetails);
            let items = self.menu_items(menu);
            let width = rows[1].width.saturating_sub(4).min(64);
            let description = Paragraph::new(text).wrap(Wrap { trim: false });
            let text_height =
                u16::try_from(description.line_count(width.saturating_sub(2))).unwrap_or(u16::MAX);
            let inner = popup(
                frame,
                rows[1],
                title,
                if destructive { ERROR } else { ACCENT },
                width,
                text_height + items.len() as u16 + 3,
            );
            let areas = Layout::vertical([
                Constraint::Length(text_height),
                Constraint::Length(1),
                Constraint::Min(items.len() as u16),
            ])
            .split(inner);
            frame.render_widget(description, areas[0]);
            frame.render_stateful_widget(
                List::new(items.into_iter().enumerate().map(|(index, label)| {
                    ListItem::new(label).style(Style::default().fg(if destructive && index == 1 {
                        ERROR
                    } else if menu == Menu::Actions && index == 0 {
                        SUCCESS
                    } else {
                        Color::White
                    }))
                }))
                .highlight_symbol("▶ ")
                .highlight_style(
                    Style::default()
                        .fg(Color::Black)
                        .bg(if destructive && self.menu_selected == 1 {
                            ERROR
                        } else {
                            ACCENT
                        })
                        .add_modifier(Modifier::BOLD),
                ),
                areas[2],
                &mut ListState::default().with_selected(Some(self.menu_selected)),
            );
        }
        let back = self.keys.hint(Command::Back);
        let confirm = self.keys.hint(Command::Confirm);
        let help = self.keys.hint(Command::Help);
        let navigation = format!(
            "{} / {}",
            self.keys.hint(Command::Up),
            self.keys.hint(Command::Down)
        );
        let footer = if self.help {
            format!("HELP · {back} / {help}: return")
        } else if self.menu.is_some() {
            format!("MENU · {navigation}: move · {confirm}: select\n{back}: back · {help}: help")
        } else if self.jump.is_some() {
            "Text or agenda number · ↑↓: select · Enter: jump · Esc: back".into()
        } else if let Some(edit) = &self.edit {
            if edit.typing {
                "TYPING · Esc: done · Enter: newline".into()
            } else {
                format!(
                    "DETAILS · {navigation}: field · {confirm}: select\n{back}: back · {help}: help"
                )
            }
        } else {
            format!(
                "PRESENTATION · {navigation}: previous/next\n{back}: menu / end meeting · {help}: help\n{confirm}: actions · PgUp/PgDn: scroll"
            )
        };
        let footer = if let Some(error) = error {
            format!("{error}\n{footer}")
        } else {
            footer
        };
        frame.render_widget(
            Paragraph::new(footer.replace('\n', " · "))
                .style(Style::default().fg(if error.is_some() { ERROR } else { ACCENT })),
            rows[2],
        );
    }
}

pub fn run_with_settings(
    terminal: &mut Terminal<impl Backend>,
    store: &Store,
    session: Session,
    settings: &Settings,
) -> io::Result<Vec<String>> {
    run_meeting(
        terminal,
        store,
        Postgame::new_with_settings(session, settings),
    )
}

pub fn run_replay_with_settings(
    terminal: &mut Terminal<impl Backend>,
    store: &Store,
    session: Session,
    settings: &Settings,
) -> io::Result<Vec<String>> {
    run_meeting(
        terminal,
        store,
        Postgame::new_replay_with_settings(session, settings, store)?,
    )
}

fn run_meeting(
    terminal: &mut Terminal<impl Backend>,
    store: &Store,
    mut app: Postgame,
) -> io::Result<Vec<String>> {
    let mut last = Instant::now();
    loop {
        let now = Instant::now();
        app.tick_replay(now.duration_since(last), store);
        last = now;
        terminal.draw(|frame| app.draw(frame))?;
        let delay = if app.replay.as_ref().is_some_and(|replay| !replay.paused) {
            10
        } else {
            100
        };
        if !event::poll(Duration::from_millis(delay))? {
            continue;
        }
        match event::read()? {
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                if let Some(links) = app.key(key, store) {
                    return Ok(links);
                }
                let current = app.current_index();
                app.sync_replay(current, store);
            }
            Event::Paste(text) => app.paste(&text),
            _ => {}
        }
    }
}

fn presentation_time(session: &Session, index: usize) -> String {
    let entry = &session.entries[index];
    let mut time = format!(
        "Wall {} · GC {}",
        entry.timestamp.format("%H:%M:%S"),
        entry
            .gc_seconds_remaining
            .map(format_clock)
            .unwrap_or_else(|| "—".into()),
    );
    if let Some(phase) = presentation_phase(session, index) {
        write!(time, " · {phase}").unwrap();
    }
    time
}

fn presentation_phase(session: &Session, index: usize) -> Option<&'static str> {
    let mut game = session.match_type.unwrap_or(MatchType::Official);
    let mut phase = None;
    for entry in session.entries[..=index]
        .iter()
        .filter(|entry| !entry.undone)
    {
        let entry_game = entry.game.unwrap_or_else(|| {
            if session.match_type.is_none() && entry.kind == EventKind::GoldenGoalStart {
                MatchType::GoldenGoal
            } else if session.match_type.is_none() && entry.ends_golden_goal() {
                MatchType::Official
            } else {
                game
            }
        });
        if entry_game != game {
            phase = None;
            game = entry_game;
        }
        if game != MatchType::Official {
            continue;
        }
        phase = if entry.has_controller_snapshot() {
            entry.controller_phase()
        } else {
            match entry.kind {
                EventKind::GameStart => Some("first half"),
                EventKind::HalfTime => Some("half-time"),
                EventKind::SecondHalfStart => Some("second half"),
                EventKind::GameEnd => None,
                _ => phase,
            }
        };
    }
    phase
}

fn clock(entry: &Entry) -> String {
    entry
        .gc_seconds_remaining
        .map(format_clock)
        .map(|clock| format!("GC {clock}"))
        .unwrap_or_else(|| format!("{} (GC unavailable)", entry.timestamp.format("%H:%M:%S")))
}

fn issue_title(entry: &Entry, note: &PostgameNote) -> String {
    if !note.title.trim().is_empty() {
        return note.title.lines().collect::<Vec<_>>().join(" ");
    }
    entry
        .text
        .lines()
        .next()
        .filter(|text| !text.is_empty())
        .unwrap_or(entry.kind.label())
        .chars()
        .take(100)
        .collect()
}

fn game_label(game: Option<MatchType>) -> &'static str {
    match game {
        Some(MatchType::GoldenGoal) => "Golden Goal",
        Some(MatchType::Official) => "Official match",
        Some(MatchType::Combined) => "Golden Goal + official",
        None => "Legacy session (game unspecified)",
    }
}

fn issue_url(session: &Session, index: usize, entry: &Entry, note: &PostgameNote) -> String {
    let game = entry.game.or(session.match_type);
    let (home, away) = session.teams_for(entry_game(session, index));
    let assignments = session
        .metadata
        .robots
        .iter()
        .enumerate()
        .map(|(index, name)| format!("{}: {}", index + 1, name))
        .collect::<Vec<_>>()
        .join(", ");
    let body = format!(
        "## Match context\n\n{} — {} vs {}\nDate: {}\nMatch: {}\nGame: {}\n\nEvent: {}\nWall-clock timestamp: {}\n{}\n\nRobots involved: {}\nMatch robot assignments (player: robot): {}\n\nVolunteer: {} (assign on GitHub, then remove this line)\n\n## Note\n{}\n\n## Discussion / decision\n{}\n",
        session.metadata.event,
        home,
        away,
        session.metadata.date,
        session.metadata.label,
        game_label(game),
        entry.kind.label(),
        entry.timestamp.to_rfc3339(),
        clock(entry),
        note.robots,
        assignments,
        note.owner,
        entry.text,
        note.discussion
    );
    let title = format!("post-game task: {}", issue_title(entry, note));
    format!(
        "https://github.com/HULKs/hulk/issues/new?title={}&type=task&labels=is%3APostgameNotes%2CGO26&projects=HULKs%2F2&body={}",
        encode_query(&title),
        encode_query(&body)
    )
}

fn entry_game(session: &Session, index: usize) -> MatchType {
    if let Some(game) = session.entries[index].game {
        return game;
    }
    if let Some(game) = session.match_type {
        return if game == MatchType::Combined {
            MatchType::Official
        } else {
            game
        };
    }
    let mut game = MatchType::Official;
    for entry in session.entries[..=index]
        .iter()
        .filter(|entry| !entry.undone)
    {
        match entry.kind {
            EventKind::GoldenGoalStart => game = MatchType::GoldenGoal,
            _ if entry.ends_golden_goal() => game = MatchType::Official,
            _ => {}
        }
    }
    let entry = &session.entries[index];
    if game == MatchType::GoldenGoal
        && entry.from_controller
        && entry.kind != EventKind::GoldenGoalStart
    {
        MatchType::Official
    } else {
        game
    }
}

fn encode_query(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(char::from(byte));
        } else {
            write!(encoded, "%{byte:02X}").unwrap();
        }
    }
    encoded
}

pub fn open_tabs(links: &[String]) -> io::Result<()> {
    if !cfg!(target_os = "macos") && !links.is_empty() {
        return Err(io::Error::other(
            "Opening Safari tabs requires macOS; your action items remain saved.",
        ));
    }
    for link in links {
        let status = ProcessCommand::new("open")
            .args(["-a", "Safari", link])
            .status()?;
        if !status.success() {
            return Err(io::Error::other(
                "Safari could not open a tab; action items remain saved. Reopen postgame to retry.",
            ));
        }
    }
    Ok(())
}
