use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Command {
    Up,
    Down,
    Left,
    Right,
    Confirm,
    Back,
    Help,
}

pub(crate) fn wrap_selection(selected: usize, len: usize, forward: bool) -> usize {
    if len == 0 {
        return 0;
    }
    if forward {
        (selected + 1) % len
    } else {
        (selected + len - 1) % len
    }
}

pub const COMMANDS: [Command; 7] = [
    Command::Up,
    Command::Down,
    Command::Left,
    Command::Right,
    Command::Confirm,
    Command::Back,
    Command::Help,
];

impl Command {
    pub fn label(self) -> &'static str {
        match self {
            Self::Up => "Up",
            Self::Down => "Down",
            Self::Left => "Left",
            Self::Right => "Right",
            Self::Confirm => "Confirm",
            Self::Back => "Back",
            Self::Help => "Help",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Keybindings {
    pub up: Vec<String>,
    pub down: Vec<String>,
    pub left: Vec<String>,
    pub right: Vec<String>,
    pub confirm: Vec<String>,
    pub back: Vec<String>,
    #[serde(default, skip_serializing, rename = "menu")]
    pub(crate) legacy_menu: Vec<String>,
    pub help: Vec<String>,
}

impl Default for Keybindings {
    fn default() -> Self {
        Self {
            up: ["Up", "k", "w"].map(String::from).into(),
            down: ["Down", "j", "s"].map(String::from).into(),
            left: ["Left", "h", "a"].map(String::from).into(),
            right: ["Right", "l", "d"].map(String::from).into(),
            confirm: ["Enter", "Space"].map(String::from).into(),
            back: vec!["Esc".into()],
            legacy_menu: Vec::new(),
            help: vec!["?".into()],
        }
    }
}

impl Keybindings {
    pub fn command(&self, key: KeyEvent) -> Option<Command> {
        if key.kind == KeyEventKind::Release || matches!(key.code, KeyCode::Tab | KeyCode::BackTab)
        {
            return None;
        }
        let key = normalize(key.code, key.modifiers);
        COMMANDS.into_iter().find(|command| {
            self.bindings(*command)
                .iter()
                .any(|binding| parse(binding).is_ok_and(|parsed| parsed == key))
        })
    }

    pub fn bindings(&self, command: Command) -> &[String] {
        match command {
            Command::Up => &self.up,
            Command::Down => &self.down,
            Command::Left => &self.left,
            Command::Right => &self.right,
            Command::Confirm => &self.confirm,
            Command::Back => &self.back,
            Command::Help => &self.help,
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        let mut used = HashMap::new();
        for command in COMMANDS {
            let bindings = self.bindings(command);
            if bindings.is_empty() {
                return Err(format!("{} needs at least one key", command.label()));
            }
            for binding in bindings {
                let key =
                    parse(binding).map_err(|error| format!("{}: {error}", command.label()))?;
                if let Some(previous) = used.insert(key, command) {
                    return Err(format!(
                        "{} is bound to both {} and {}",
                        display(key),
                        previous.label(),
                        command.label(),
                    ));
                }
            }
        }
        Ok(())
    }

    pub fn hint(&self, command: Command) -> String {
        self.bindings(command)
            .iter()
            .take(1)
            .map(|binding| {
                parse(binding)
                    .map(display)
                    .unwrap_or_else(|_| binding.clone())
            })
            .collect::<Vec<_>>()
            .join(" / ")
    }
}

fn parse(binding: &str) -> Result<(KeyCode, KeyModifiers), String> {
    let mut name = binding;
    let mut modifiers = KeyModifiers::NONE;
    while let Some((prefix, rest)) = name.split_once('+') {
        let modifier = match prefix.to_ascii_lowercase().as_str() {
            "ctrl" => KeyModifiers::CONTROL,
            "alt" => KeyModifiers::ALT,
            "shift" => KeyModifiers::SHIFT,
            _ => break,
        };
        if modifiers.contains(modifier) {
            return Err(format!("Repeated modifier in {binding:?}"));
        }
        modifiers.insert(modifier);
        name = rest;
    }
    let code = match name.to_ascii_lowercase().as_str() {
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "enter" => KeyCode::Enter,
        "space" => KeyCode::Char(' '),
        "esc" => KeyCode::Esc,
        "tab" | "backtab" => {
            return Err(
                "Tab and BackTab are no longer supported as keybindings; choose another key".into(),
            );
        }
        "backspace" => KeyCode::Backspace,
        "delete" => KeyCode::Delete,
        "insert" => KeyCode::Insert,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pageup" => KeyCode::PageUp,
        "pagedown" => KeyCode::PageDown,
        _ => {
            let mut chars = name.chars();
            match (chars.next(), chars.next()) {
                (Some(character), None) if !character.is_control() => KeyCode::Char(character),
                _ => match name
                    .strip_prefix(['f', 'F'])
                    .and_then(|number| number.parse::<u8>().ok())
                    .filter(|number| (1..=24).contains(number))
                {
                    Some(number) => KeyCode::F(number),
                    None => return Err(format!("Unsupported key {binding:?}")),
                },
            }
        }
    };
    if let KeyCode::Char(character) = code {
        if modifiers.contains(KeyModifiers::SHIFT)
            && (!character.is_ascii_alphabetic() || modifiers.contains(KeyModifiers::CONTROL))
        {
            return Err("This Shift spelling is not reliably distinguishable in a terminal; use the resulting character or an unshifted Ctrl binding".into());
        }
        if modifiers.contains(KeyModifiers::CONTROL) {
            let alias = match character.to_ascii_lowercase() {
                'i' => Some("Tab"),
                'm' => Some("Enter"),
                '[' => Some("Esc"),
                '?' => Some("Backspace"),
                _ => None,
            };
            if let Some(alias) = alias {
                return Err(format!(
                    "{binding} is indistinguishable from {alias}; use {alias} instead"
                ));
            }
            if !character.is_ascii_alphabetic() && !matches!(character, ' ' | '4'..='7') {
                return Err(
                    "Unsupported terminal Ctrl character; use a letter, Space, or 4–7".into(),
                );
            }
        }
    }
    let key = normalize(code, modifiers);
    if matches!(key.0, KeyCode::Char('c' | 'C')) && key.1.contains(KeyModifiers::CONTROL) {
        return Err("Ctrl+C is reserved for emergency exit".into());
    }
    Ok(key)
}

fn normalize(mut code: KeyCode, mut modifiers: KeyModifiers) -> (KeyCode, KeyModifiers) {
    if let KeyCode::Char(character) = code {
        if modifiers.contains(KeyModifiers::CONTROL) {
            code = KeyCode::Char(character.to_ascii_lowercase());
        } else if modifiers.contains(KeyModifiers::SHIFT) {
            code = KeyCode::Char(character.to_ascii_uppercase());
        }
        // Terminals disagree about reporting Shift for already-shifted printable characters.
        modifiers.remove(KeyModifiers::SHIFT);
    } else if code == KeyCode::BackTab {
        code = KeyCode::Tab;
        modifiers.insert(KeyModifiers::SHIFT);
    }
    (code, modifiers)
}

fn display((code, modifiers): (KeyCode, KeyModifiers)) -> String {
    let mut label = String::new();
    for (modifier, prefix) in [
        (KeyModifiers::CONTROL, "Ctrl+"),
        (KeyModifiers::ALT, "Alt+"),
        (KeyModifiers::SHIFT, "Shift+"),
    ] {
        if modifiers.contains(modifier) {
            label.push_str(prefix);
        }
    }
    label.push_str(&match code {
        KeyCode::Up => "Up".into(),
        KeyCode::Down => "Down".into(),
        KeyCode::Left => "Left".into(),
        KeyCode::Right => "Right".into(),
        KeyCode::Enter => "Enter".into(),
        KeyCode::Char(' ') => "Space".into(),
        KeyCode::Char(character) => character.to_string(),
        KeyCode::Esc => "Esc".into(),
        KeyCode::Tab => "Tab".into(),
        KeyCode::Backspace => "Backspace".into(),
        KeyCode::Delete => "Delete".into(),
        KeyCode::Insert => "Insert".into(),
        KeyCode::Home => "Home".into(),
        KeyCode::End => "End".into(),
        KeyCode::PageUp => "PageUp".into(),
        KeyCode::PageDown => "PageDown".into(),
        KeyCode::F(number) => format!("F{number}"),
        _ => unreachable!("Only parsed keybindings are displayed"),
    });
    label
}
