use std::{
    fs, io,
    path::{Path, PathBuf},
    time::Duration,
};

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::{
    Terminal,
    backend::Backend,
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style},
    widgets::{Block, List, ListItem, ListState, Padding, Paragraph, Wrap},
};
use tui_textarea::TextArea;

use crate::{
    discovery::Discovery,
    navigation::{COMMANDS, Command, Keybindings, wrap_selection},
    presentation::{ACCENT, ERROR, FOCUS, MUTED, SUCCESS, popup, solid_panel},
    settings::Settings,
    storage::Store,
    teams::{TEAMS, team_number},
};

pub(crate) enum Launch {
    New,
    Resume(PathBuf),
    Postgame(Option<PathBuf>),
}

pub(crate) fn run(
    terminal: &mut Terminal<impl Backend>,
    settings: &mut Settings,
) -> io::Result<Option<Launch>> {
    let recent = match crate::recent::list() {
        Ok(paths) => paths,
        Err(error) => {
            show_error(
                terminal,
                &settings.keys,
                &format!("Cannot read recent sessions: {error}"),
            )?;
            Vec::new()
        }
    };
    let mut selected = 0;
    loop {
        let mut items = [
            "New match",
            "Resume match",
            "Postgame meeting",
            "Manage sessions / recordings",
            "Settings",
        ]
        .map(str::to_owned)
        .to_vec();
        let recent_start = items.len();
        items.extend(recent.iter().map(|path| {
            format!(
                "Recent: {}",
                path.file_name()
                    .unwrap_or(path.as_os_str())
                    .to_string_lossy()
            )
        }));
        items.push("Exit".into());
        let choice = {
            let mut discovery = Discovery::default();
            choose_with_discovery(
                terminal,
                "RoboCup logger / Home",
                &items,
                &settings.keys,
                &mut selected,
                "",
                None,
                Some(&mut discovery),
                false,
            )?
        };
        match choice {
            Some(0) => return Ok(Some(Launch::New)),
            Some(1 | 2) => {
                if let Some(path) = pick_session(terminal, settings)? {
                    return Ok(Some(if choice == Some(1) {
                        Launch::Resume(path)
                    } else {
                        Launch::Postgame(Some(path))
                    }));
                }
            }
            Some(3) => {
                if let Some(path) = pick_session(terminal, settings)? {
                    manage(terminal, settings, &path)?;
                }
            }
            Some(4) => edit_settings(terminal, settings)?,
            Some(index) if (recent_start..recent_start + recent.len()).contains(&index) => {
                let path = &recent[index - recent_start];
                match choose(
                    terminal,
                    "Home / Recent session",
                    &["Resume match", "Postgame meeting", "Cancel"].map(str::to_owned),
                    &settings.keys,
                    &mut 2,
                    &path.display().to_string(),
                    Some(("RoboCup logger / Home", &items, selected, false)),
                )? {
                    Some(0) => return Ok(Some(Launch::Resume(path.clone()))),
                    Some(1) => return Ok(Some(Launch::Postgame(Some(path.clone())))),
                    _ => {}
                }
            }
            _ => {
                if choose(
                    terminal,
                    "Home / Exit",
                    &["Cancel", "Quit logger"].map(str::to_owned),
                    &settings.keys,
                    &mut 0,
                    "Are you sure you want to quit?",
                    Some(("RoboCup logger / Home", &items, selected, false)),
                )? == Some(1)
                {
                    return Ok(None);
                }
            }
        }
    }
}

fn visible(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() && c != '\n' { ' ' } else { c })
        .collect()
}

fn footer(keys: &Keybindings) -> String {
    format!(
        "{}/{} move · {} select · {} back · {} help",
        keys.hint(Command::Up),
        keys.hint(Command::Down),
        keys.hint(Command::Confirm),
        keys.hint(Command::Back),
        keys.hint(Command::Help)
    )
}

fn choose(
    terminal: &mut Terminal<impl Backend>,
    title: &str,
    items: &[String],
    keys: &Keybindings,
    selected: &mut usize,
    description: &str,
    backdrop: Option<(&str, &[String], usize, bool)>,
) -> io::Result<Option<usize>> {
    choose_with_discovery(
        terminal,
        title,
        items,
        keys,
        selected,
        description,
        backdrop,
        None,
        false,
    )
}

fn choose_searchable(
    terminal: &mut Terminal<impl Backend>,
    title: &str,
    items: &[String],
    keys: &Keybindings,
    selected: &mut usize,
    description: &str,
    backdrop: Option<(&str, &[String], usize, bool)>,
) -> io::Result<Option<usize>> {
    choose_with_discovery(
        terminal,
        title,
        items,
        keys,
        selected,
        description,
        backdrop,
        None,
        true,
    )
}

#[derive(Default)]
struct MenuSearch {
    query: String,
    editing: bool,
}

impl MenuSearch {
    fn indices(&self, items: &[String]) -> Vec<usize> {
        if self.query.is_empty() {
            return (0..items.len()).collect();
        }
        let query = self.query.to_lowercase();
        items
            .iter()
            .enumerate()
            .filter(|(_, item)| item.as_str() == "../" || item.to_lowercase().contains(&query))
            .map(|(index, _)| index)
            .collect()
    }

    fn input(&mut self, key: KeyEvent, items: &[String], selected: &mut usize) -> bool {
        if key.code == KeyCode::Esc && (self.editing || !self.query.is_empty()) {
            self.editing = false;
            self.query.clear();
            return true;
        }
        if !self.editing {
            if key.code == KeyCode::Char('/')
                && !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
            {
                self.editing = true;
                return true;
            }
            return false;
        }
        match key.code {
            KeyCode::Enter => self.editing = false,
            KeyCode::Backspace => {
                self.query.pop();
            }
            KeyCode::Char(character)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.query.push(character);
            }
            _ => return true,
        }
        if self.editing {
            let indices = self.indices(items);
            // Keep ../ reachable without making it the default result of every search.
            *selected = indices
                .iter()
                .copied()
                .find(|&index| items[index] != "../")
                .or_else(|| indices.first().copied())
                .unwrap_or(0);
        }
        true
    }
}

#[allow(clippy::too_many_arguments)]
fn choose_with_discovery(
    terminal: &mut Terminal<impl Backend>,
    title: &str,
    items: &[String],
    keys: &Keybindings,
    selected: &mut usize,
    description: &str,
    backdrop: Option<(&str, &[String], usize, bool)>,
    mut discovery: Option<&mut Discovery>,
    searchable: bool,
) -> io::Result<Option<usize>> {
    *selected = (*selected).min(items.len().saturating_sub(1));
    let searchable = searchable
        && keys
            .command(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE))
            .is_none();
    let dropdown = backdrop.is_some_and(|(_, _, _, dropdown)| dropdown);
    let mut search = MenuSearch::default();
    loop {
        let indices = search.indices(items);
        let filtered: Vec<_> = indices.iter().map(|&index| items[index].clone()).collect();
        let position = indices
            .iter()
            .position(|&index| index == *selected)
            .unwrap_or(0);
        if let Some(&index) = indices.get(position) {
            *selected = index;
        }
        if let Some(discovery) = discovery.as_mut() {
            discovery.poll();
        }
        let mut visible_menu = false;
        terminal.draw(|frame| {
            visible_menu = frame.area().width >= 40 && frame.area().height >= 12;
            if !visible_menu {
                frame.render_widget(
                    Paragraph::new(
                        "Resize to at least 40 × 12. Selection is disabled; Back cancels.",
                    )
                    .wrap(Wrap { trim: false }),
                    frame.area(),
                );
                return;
            }
            let header_height = if discovery.is_some() && description.is_empty() {
                1
            } else if backdrop.is_some() || description.is_empty() {
                0
            } else {
                3
            };
            let rows = Layout::vertical([
                Constraint::Length(header_height),
                Constraint::Min(1),
                Constraint::Length(1),
            ])
            .split(frame.area());
            frame.render_widget(
                Block::default().style(Style::default().bg(Color::Black)),
                frame.area(),
            );
            let network_status = discovery.as_ref().map(|discovery| discovery.status());
            frame.render_widget(
                Paragraph::new(if let Some((status, _)) = &network_status {
                    format!("GC logging OFF • {status}\n{}", visible(description))
                } else if backdrop.is_some() {
                    String::new()
                } else {
                    visible(description)
                })
                .style(Style::default().fg(match network_status {
                    Some((_, true)) => SUCCESS,
                    Some((_, false)) => FOCUS,
                    None => MUTED,
                }))
                .wrap(Wrap { trim: false }),
                rows[0],
            );
            let (base_title, base_items, base_selected, _) =
                backdrop.unwrap_or((title, &filtered, position, false));
            let base_inner = solid_panel(
                frame,
                rows[1],
                Block::bordered()
                    .padding(Padding::new(
                        2,
                        2,
                        u16::from(rows[1].height >= 10),
                        u16::from(rows[1].height >= 10),
                    ))
                    .title(visible(base_title)),
                ACCENT,
            );
            frame.render_stateful_widget(
                List::new(base_items.iter().map(|item| ListItem::new(visible(item))))
                    .style(Style::default().fg(Color::Gray))
                    .highlight_symbol("▶ ")
                    .highlight_style(
                        Style::default()
                            .fg(Color::Black)
                            .bg(ACCENT)
                            .add_modifier(Modifier::BOLD),
                    ),
                base_inner,
                &mut ListState::default()
                    .with_selected((!base_items.is_empty()).then_some(base_selected)),
            );
            if backdrop.is_none() && filtered.is_empty() {
                frame.render_widget(
                    Paragraph::new("No matches. / edits search; Esc clears it."),
                    base_inner,
                );
            }
            if backdrop.is_some() {
                let description = Paragraph::new(visible(description)).wrap(Wrap { trim: false });
                let description_height =
                    u16::try_from(description.line_count(rows[1].width.saturating_sub(4).min(62)))
                        .unwrap_or(u16::MAX);
                let list_height = if dropdown {
                    12.min(filtered.len().max(1) as u16)
                } else {
                    filtered.len() as u16
                };
                let content_height = description_height.saturating_add(list_height);
                let inner = popup(
                    frame,
                    rows[1],
                    title,
                    FOCUS,
                    64,
                    content_height.saturating_add(2),
                );
                visible_menu = if dropdown {
                    inner.height > description_height
                } else {
                    inner.height >= content_height
                };
                let sections =
                    Layout::vertical([Constraint::Length(description_height), Constraint::Min(0)])
                        .split(inner);
                frame.render_widget(
                    description.style(Style::default().fg(Color::White)),
                    sections[0],
                );
                frame.render_stateful_widget(
                    List::new(filtered.iter().map(|item| ListItem::new(visible(item))))
                        .style(Style::default().fg(Color::Gray))
                        .highlight_symbol("▶ ")
                        .highlight_style(
                            Style::default()
                                .fg(Color::Black)
                                .bg(ACCENT)
                                .add_modifier(Modifier::BOLD),
                        ),
                    sections[1],
                    &mut ListState::default()
                        .with_selected((!filtered.is_empty()).then_some(position)),
                );
                if filtered.is_empty() {
                    frame.render_widget(
                        Paragraph::new("No matches. / edits search; Esc clears it."),
                        sections[1],
                    );
                }
            }
            frame.render_widget(
                Paragraph::new(if visible_menu {
                    if search.editing {
                        format!(
                            "Search: {} · Enter: done · Esc: clear",
                            visible(&search.query)
                        )
                    } else if searchable {
                        format!(
                            "{} · / search: {} · {} matches",
                            footer(keys),
                            visible(&search.query),
                            filtered.len()
                        )
                    } else {
                        footer(keys)
                    }
                } else {
                    "Resize to show the complete confirmation. Back cancels.".into()
                })
                .style(Style::default().fg(if visible_menu {
                    MUTED
                } else {
                    FOCUS
                })),
                rows[2],
            );
        })?;
        if !event::poll(Duration::from_millis(100))? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind == KeyEventKind::Release {
            continue;
        }
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return Ok(None);
        }
        if searchable && search.input(key, items, selected) {
            continue;
        }
        if matches!(key.code, KeyCode::Tab | KeyCode::BackTab) {
            continue;
        }
        let command = if dropdown {
            match key.code {
                KeyCode::Enter | KeyCode::Char(' ') => Some(Command::Confirm),
                KeyCode::Esc => Some(Command::Back),
                KeyCode::Up | KeyCode::Left => Some(Command::Up),
                KeyCode::Down | KeyCode::Right => Some(Command::Down),
                _ => keys.command(key),
            }
        } else {
            keys.command(key)
        };
        match command {
            Some(Command::Left | Command::Right) if dropdown => {
                let forward = command == Some(Command::Right);
                if let Some(&index) = indices.get(wrap_selection(position, indices.len(), forward))
                {
                    *selected = index;
                }
            }
            Some(Command::Up | Command::Down) => {
                let forward = command == Some(Command::Down);
                if let Some(&index) = indices.get(wrap_selection(position, indices.len(), forward))
                {
                    *selected = index;
                }
            }
            Some(Command::Back | Command::Left) => return Ok(None),
            Some(Command::Confirm | Command::Right) if visible_menu && !indices.is_empty() => {
                return Ok(Some(indices[position]));
            }
            Some(Command::Help) => help(terminal, keys)?,
            _ => {}
        }
    }
}

fn help(terminal: &mut Terminal<impl Backend>, keys: &Keybindings) -> io::Result<()> {
    let bindings = COMMANDS
        .iter()
        .map(|&command| format!("{}: {}", command.label(), keys.hint(command)))
        .collect::<Vec<_>>()
        .join("\n");
    message(
        terminal,
        keys,
        ACCENT,
        "Navigation help",
        &format!(
            "{bindings}\n\nEditing: Enter/Esc finishes the field.\nLive note: Enter starts; Enter/Esc saves.\nCtrl+C: exit."
        ),
    )
}

pub(crate) fn show_error(
    terminal: &mut Terminal<impl Backend>,
    keys: &Keybindings,
    text: &str,
) -> io::Result<()> {
    message(terminal, keys, ERROR, "Operation failed", text)
}

fn message(
    terminal: &mut Terminal<impl Backend>,
    keys: &Keybindings,
    accent: Color,
    title: &str,
    text: &str,
) -> io::Result<()> {
    let text = visible(text);
    let mut scroll = 0u16;
    loop {
        terminal.draw(|frame| {
            let rows =
                Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(frame.area());
            let inner = solid_panel(
                frame,
                rows[0],
                Block::bordered()
                    .padding(Padding::new(
                        2,
                        2,
                        u16::from(rows[0].height >= 7),
                        u16::from(rows[0].height >= 7),
                    ))
                    .title(visible(title)),
                accent,
            );
            let paragraph = Paragraph::new(text.as_str())
                .style(Style::default().fg(Color::White).bg(Color::Black))
                .wrap(Wrap { trim: false });
            let max = paragraph
                .line_count(inner.width)
                .saturating_sub(usize::from(inner.height));
            scroll = scroll.min(u16::try_from(max).unwrap_or(u16::MAX));
            frame.render_widget(paragraph.scroll((scroll, 0)), inner);
            frame.render_widget(
                Paragraph::new(footer(keys))
                    .style(Style::default().fg(MUTED).bg(Color::Black))
                    .wrap(Wrap { trim: false }),
                rows[1],
            );
        })?;
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind == KeyEventKind::Release {
            continue;
        }
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return Ok(());
        }
        match keys.command(key) {
            Some(Command::Up) => scroll = scroll.saturating_sub(3),
            Some(Command::Down) => scroll = scroll.saturating_add(3),
            Some(Command::Back | Command::Confirm | Command::Left) => return Ok(()),
            _ => {}
        }
    }
}

struct FileChoice {
    path: PathBuf,
    directory: bool,
    label: String,
}

fn session_files(directory: &Path) -> io::Result<Vec<FileChoice>> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name();
        if name.to_string_lossy().starts_with('.') {
            continue;
        }
        let path = entry.path();
        let metadata = match fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        let directory = metadata.is_dir();
        if directory || (metadata.is_file() && path.extension().is_some_and(|ext| ext == "json")) {
            entries.push(FileChoice {
                label: format!(
                    "{}{}",
                    name.to_string_lossy(),
                    if directory { "/" } else { "" }
                ),
                path,
                directory,
            });
        }
    }
    entries.sort_by(|a, b| b.directory.cmp(&a.directory).then(a.label.cmp(&b.label)));
    if let Some(parent) = directory.parent() {
        entries.insert(
            0,
            FileChoice {
                path: parent.into(),
                directory: true,
                label: "../".into(),
            },
        );
    }
    Ok(entries)
}

pub(crate) fn pick_session(
    terminal: &mut Terminal<impl Backend>,
    settings: &Settings,
) -> io::Result<Option<PathBuf>> {
    let mut directory = if settings.log_directory.is_dir() {
        settings.log_directory.canonicalize()?
    } else {
        std::env::current_dir()?
    };
    loop {
        let entries = match session_files(&directory) {
            Ok(entries) => entries,
            Err(error) => {
                message(
                    terminal,
                    &settings.keys,
                    ERROR,
                    "Cannot browse folder",
                    &format!("{}: {error}", directory.display()),
                )?;
                return Ok(None);
            }
        };
        let items: Vec<_> = entries.iter().map(|entry| entry.label.clone()).collect();
        let Some(index) = choose_searchable(
            terminal,
            "Choose a session JSON",
            &items,
            &settings.keys,
            &mut 0,
            &format!("{}", directory.display()),
            None,
        )?
        else {
            return Ok(None);
        };
        if entries[index].directory {
            directory = entries[index].path.clone();
        } else {
            return Ok(Some(entries[index].path.clone()));
        }
    }
}

fn text_input(
    terminal: &mut Terminal<impl Backend>,
    title: &str,
    value: &str,
) -> io::Result<Option<String>> {
    let mut editor = TextArea::new(vec![value.into()]);
    editor.set_style(Style::default().fg(FOCUS).bg(Color::Black));
    loop {
        terminal.draw(|frame| {
            frame.render_widget(
                Block::default().style(Style::default().bg(Color::Black)),
                frame.area(),
            );
            let field_height = if frame.area().height >= 12 { 5 } else { 3 };
            let rows = Layout::vertical([Constraint::Length(field_height), Constraint::Min(1)])
                .split(frame.area());
            let inner = solid_panel(
                frame,
                rows[0],
                Block::bordered().title(title).padding(Padding::new(
                    2,
                    2,
                    u16::from(field_height >= 5),
                    u16::from(field_height >= 5),
                )),
                ACCENT,
            );
            frame.render_widget(&editor, inner);
            frame.render_widget(
                Paragraph::new("EDITING · Enter/Esc: done").style(Style::default().fg(MUTED)),
                rows[1],
            );
        })?;
        match event::read()? {
            Event::Key(key) if key.kind != KeyEventKind::Release => match key.code {
                KeyCode::Tab | KeyCode::BackTab => {}
                KeyCode::Enter | KeyCode::Esc => return Ok(Some(editor.lines().join(""))),
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return Ok(None);
                }
                _ => {
                    editor.input(key);
                }
            },
            Event::Paste(text) => {
                editor.insert_str(
                    text.chars()
                        .map(|c| if c.is_control() { ' ' } else { c })
                        .collect::<String>(),
                );
            }
            _ => {}
        }
    }
}

const SETTING_LABELS: [&str; 11] = [
    "Log folder",
    "Python interpreter",
    "Whisper model cache",
    "Default event",
    "Tracked team",
    "Opponent",
    "Robot 1",
    "Robot 2",
    "Robot 3",
    "Robot 4",
    "Robot 5",
];

fn setting_values(settings: &Settings) -> Vec<String> {
    let mut values = vec![
        settings.log_directory.display().to_string(),
        settings.python.display().to_string(),
        settings.model_cache.display().to_string(),
        settings.event.clone(),
        settings.home.clone(),
        settings.away.clone(),
    ];
    values.extend(settings.robots.iter().cloned());
    values
}

fn set_field(settings: &mut Settings, index: usize, value: String) {
    match index {
        0 => settings.log_directory = value.into(),
        1 => settings.python = value.into(),
        2 => settings.model_cache = value.into(),
        3 => settings.event = value,
        4 => settings.home = value,
        5 => settings.away = value,
        6..=10 => settings.robots[index - 6] = value,
        _ => unreachable!("settings field index"),
    }
}

fn edit_settings(terminal: &mut Terminal<impl Backend>, settings: &mut Settings) -> io::Result<()> {
    let mut draft = settings.clone();
    let mut selected = 0;
    loop {
        let values = setting_values(&draft);
        let mut items: Vec<_> = SETTING_LABELS
            .iter()
            .zip(&values)
            .map(|(label, value)| format!("{label}: {value}"))
            .collect();
        items.extend(["Keybindings…", "Save settings", "Cancel changes"].map(str::to_owned));
        let choice = choose(
            terminal,
            "Home / Settings",
            &items,
            &settings.keys,
            &mut selected,
            "",
            None,
        )?;
        match choice {
            Some(choice @ (4 | 5)) => {
                let teams: Vec<_> = TEAMS
                    .iter()
                    .map(|(number, name)| format!("{name} (#{number})"))
                    .collect();
                let mut team = TEAMS
                    .iter()
                    .position(|(number, _)| Some(*number) == team_number(&values[choice]))
                    .unwrap_or(0);
                if let Some(team) = choose_searchable(
                    terminal,
                    SETTING_LABELS[choice],
                    &teams,
                    &settings.keys,
                    &mut team,
                    "",
                    Some(("Home / Settings", &items, selected, true)),
                )? {
                    set_field(&mut draft, choice, TEAMS[team].1.to_owned());
                }
            }
            Some(choice @ 0..=10) => {
                if let Some(value) = text_input(terminal, SETTING_LABELS[choice], &values[choice])?
                {
                    set_field(&mut draft, choice, value);
                }
            }
            Some(11) => edit_bindings(terminal, &settings.keys, &mut draft.keys)?,
            Some(12) => match draft.save() {
                Ok(()) => {
                    *settings = draft;
                    return Ok(());
                }
                Err(error) => message(
                    terminal,
                    &settings.keys,
                    ERROR,
                    "Settings not saved",
                    &error.to_string(),
                )?,
            },
            _ => {
                if draft == *settings
                    || choose(
                        terminal,
                        "Settings / Discard changes",
                        &["Cancel", "Discard changes"].map(str::to_owned),
                        &settings.keys,
                        &mut 0,
                        "Discard unsaved settings changes? Cancel keeps the draft.",
                        Some(("Home / Settings", &items, selected, false)),
                    )? == Some(1)
                {
                    return Ok(());
                }
            }
        }
    }
}

fn edit_bindings(
    terminal: &mut Terminal<impl Backend>,
    active: &Keybindings,
    draft: &mut Keybindings,
) -> io::Result<()> {
    let mut values: Vec<Vec<String>> = COMMANDS
        .iter()
        .map(|&command| draft.bindings(command).to_vec())
        .collect();
    let original = values.clone();
    let mut selected = 0;
    loop {
        let mut items: Vec<_> = COMMANDS
            .iter()
            .zip(&values)
            .map(|(command, bindings)| format!("{}: {}", command.label(), bindings.join(" ")))
            .collect();
        items.extend(
            [
                "Apply bindings to settings draft",
                "Reset bindings to defaults",
                "Cancel binding changes",
            ]
            .map(str::to_owned),
        );
        let choice = choose(
            terminal,
            "Settings / Keybindings",
            &items,
            active,
            &mut selected,
            "Aliases: Up k w · Enter Space · Ctrl+x",
            None,
        )?;
        match choice {
            Some(choice @ 0..=6) => {
                if let Some(value) = text_input(
                    terminal,
                    COMMANDS[choice].label(),
                    &values[choice].join(" "),
                )? {
                    values[choice] = value.split_whitespace().map(str::to_owned).collect();
                }
            }
            Some(7) => {
                let candidate = Keybindings {
                    up: values[0].clone(),
                    down: values[1].clone(),
                    left: values[2].clone(),
                    right: values[3].clone(),
                    confirm: values[4].clone(),
                    back: values[5].clone(),
                    help: values[6].clone(),
                    ..draft.clone()
                };
                match candidate.validate() {
                    Ok(()) => {
                        *draft = candidate;
                        return Ok(());
                    }
                    Err(error) => message(
                        terminal,
                        active,
                        ERROR,
                        "Bindings not applied",
                        &error.to_string(),
                    )?,
                }
            }
            Some(8) => {
                let defaults = Keybindings::default();
                values = COMMANDS
                    .iter()
                    .map(|&command| defaults.bindings(command).to_vec())
                    .collect();
            }
            _ => {
                if values == original
                    || choose(
                        terminal,
                        "Keybindings / Discard changes",
                        &["Cancel", "Discard changes"].map(str::to_owned),
                        active,
                        &mut 0,
                        "Discard unsaved binding changes? Cancel keeps the draft.",
                        Some(("Settings / Keybindings", &items, selected, false)),
                    )? == Some(1)
                {
                    return Ok(());
                }
            }
        }
    }
}

fn manage(
    terminal: &mut Terminal<impl Backend>,
    settings: &Settings,
    path: &Path,
) -> io::Result<()> {
    let mut selected = 0;
    loop {
        let items = [
            "Regenerate match Markdown",
            "Rebuild missing GC statistics",
            "Browse meeting recordings / transcripts",
            "Open log folder",
            "Back",
        ]
        .map(str::to_owned);
        let Some(choice) = choose(
            terminal,
            "Home / Manage",
            &items,
            &settings.keys,
            &mut selected,
            &path.display().to_string(),
            None,
        )?
        else {
            return Ok(());
        };
        let result = match choice {
            0 => Store::export(path).map(|path| {
                format!(
                    "Markdown regenerated:\n{}\nSession JSON unchanged.",
                    path.display()
                )
            }),
            1 => {
                let confirmation =
                    ["Cancel", "Rebuild and save JSON + Markdown"].map(str::to_owned);
                if choose(
                    terminal,
                    "Confirm rebuild",
                    &confirmation,
                    &settings.keys,
                    &mut 0,
                    "Fills missing statistics from raw GameController capture. Existing corrections are preserved. Close the live logger first.",
                    Some(("Home / Manage", &items, selected, false)),
                )? != Some(1)
                {
                    continue;
                }
                rebuild_file(path)
            }
            2 => {
                recordings(terminal, &settings.keys, path)?;
                continue;
            }
            3 => crate::folders::open(path).map(|()| "Opened the session log folder.".into()),
            _ => return Ok(()),
        };
        match result {
            Ok(text) => message(
                terminal,
                &settings.keys,
                SUCCESS,
                "Manage / Complete",
                &text,
            )?,
            Err(error) => message(
                terminal,
                &settings.keys,
                ERROR,
                "Manage / Failed",
                &error.to_string(),
            )?,
        }
    }
}

fn rebuild_file(path: &Path) -> io::Result<String> {
    let (store, mut session) = Store::load(path)?;
    let capture_session = session.capture_session.as_deref().ok_or_else(|| {
        io::Error::other("This session has no raw capture; missing packets cannot be recovered")
    })?;
    let packets = crate::capture::CaptureLog::read(&store.capture_path(), capture_session)?;
    let changed = crate::rebuild::rebuild(&mut session, &packets).map_err(io::Error::other)?;
    store.save(&session)?;
    Ok(format!(
        "Rebuilt {changed} statistics records.\n{}",
        store.markdown_path().display()
    ))
}

fn recordings(
    terminal: &mut Terminal<impl Backend>,
    keys: &Keybindings,
    session: &Path,
) -> io::Result<()> {
    let root = session.with_extension("postgame");
    let directories = (|| -> io::Result<Vec<PathBuf>> {
        let mut directories = Vec::new();
        for entry in fs::read_dir(&root)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                directories.push(entry.path());
            }
        }
        directories.sort();
        directories.reverse();
        Ok(directories)
    })();
    let directories = match directories {
        Ok(directories) if !directories.is_empty() => directories,
        Ok(_) => {
            return message(
                terminal,
                keys,
                FOCUS,
                "Meeting recordings",
                "No meeting recordings found.",
            );
        }
        Err(error) => {
            return message(
                terminal,
                keys,
                ERROR,
                "Meeting recordings",
                &format!("{}: {error}", root.display()),
            );
        }
    };
    let items: Vec<_> = directories
        .iter()
        .map(|path| {
            path.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    let mut selected = 0;
    while let Some(index) = choose(
        terminal,
        "Manage / Meeting recordings",
        &items,
        keys,
        &mut selected,
        "",
        None,
    )? {
        let path = &directories[index];
        view_transcript(terminal, keys, path)?;
    }
    Ok(())
}

fn view_transcript(
    terminal: &mut Terminal<impl Backend>,
    keys: &Keybindings,
    directory: &Path,
) -> io::Result<()> {
    let mut viewer = match crate::transcript_viewer::Viewer::load(directory) {
        Ok(viewer) => viewer,
        Err(error) => {
            return message(
                terminal,
                keys,
                ERROR,
                "Transcript unavailable",
                &format!(
                    "Folder: {}\n{error}\nUse Postgame → continue without recording → Retry transcription for the latest meeting.",
                    directory.display()
                ),
            );
        }
    };
    loop {
        terminal.draw(|frame| viewer.draw(frame, frame.area(), keys))?;
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind == KeyEventKind::Release {
            continue;
        }
        if (matches!(key.code, KeyCode::Char('c' | 'C'))
            && key.modifiers.contains(KeyModifiers::CONTROL))
            || viewer.key(key, keys)
        {
            return Ok(());
        }
        if keys.command(key) == Some(Command::Help) {
            message(
                terminal,
                keys,
                ACCENT,
                "Transcript controls",
                &format!(
                    "{}/{}: choose topic\n{}/{}: scroll\nPgUp/PgDn, Home/End: scroll the note and discussion\n{}: return to recordings",
                    keys.hint(Command::Up),
                    keys.hint(Command::Down),
                    keys.hint(Command::Left),
                    keys.hint(Command::Right),
                    keys.hint(Command::Back)
                ),
            )?;
        }
    }
}
