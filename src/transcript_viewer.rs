use std::{io, path::Path};

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Block, List, ListItem, ListState, Padding, Paragraph, Wrap},
};

use crate::{
    meeting::{self, Marker, NoteSnapshot, Segment, TranscriptData},
    navigation::{Command, Keybindings, wrap_selection},
    presentation::{FOCUS, MUTED, solid_panel},
};

struct Visit {
    note: Option<NoteSnapshot>,
    speech: Vec<String>,
}

struct Topic {
    source: Option<usize>,
    visits: Vec<Visit>,
}

pub(crate) struct Viewer {
    title: String,
    topics: Vec<Topic>,
    selected: usize,
    scroll: u16,
    page_height: u16,
}

impl Viewer {
    pub(crate) fn load(directory: &Path) -> io::Result<Self> {
        Ok(Self::from_data(meeting::read_transcript_data(directory)?))
    }

    fn from_data(data: TranscriptData) -> Self {
        let mut markers = data.markers;
        if markers.first().is_none_or(|marker| marker.seconds > 0.0) {
            markers.insert(
                0,
                Marker {
                    seconds: 0.0,
                    note: None,
                },
            );
        }
        let mut topics: Vec<Topic> = Vec::new();
        for (index, marker) in markers.iter().enumerate() {
            let start = marker.seconds;
            let end = markers
                .get(index + 1)
                .map_or(data.transcript.duration_seconds, |next| next.seconds);
            let speech: Vec<_> = data
                .transcript
                .segments
                .iter()
                .filter(|segment| {
                    // Match the export: crossing segments repeat; points go to the new
                    // window at a switch, or the final window at the recording's end.
                    if segment.start == segment.end {
                        segment.start >= start
                            && (segment.start < end
                                || (index + 1 == markers.len() && segment.start == end))
                    } else {
                        segment.start < end && segment.end > start && start < end
                    }
                })
                .collect();
            let visit = Visit {
                note: marker.note.clone(),
                speech: speech_paragraphs(&speech),
            };
            let source = marker.note.as_ref().map(|note| note.entry_index);
            if let Some(topic) = topics.iter_mut().find(|topic| topic.source == source) {
                topic.visits.push(visit);
            } else {
                topics.push(Topic {
                    source,
                    visits: vec![visit],
                });
            }
        }
        Self {
            title: terminal_text(&format!(
                "Transcript · {} vs {} · {}",
                data.info.metadata.home,
                data.info.metadata.away,
                data.info.started_at.format("%Y-%m-%d %H:%M")
            )),
            topics,
            selected: 0,
            scroll: 0,
            page_height: 1,
        }
    }

    pub(crate) fn key(&mut self, key: KeyEvent, keys: &Keybindings) -> bool {
        if key.kind == KeyEventKind::Release
            || matches!(key.code, KeyCode::Tab | KeyCode::BackTab)
            || (key.modifiers.contains(KeyModifiers::CONTROL)
                && matches!(key.code, KeyCode::Char('c' | 'C')))
        {
            return false;
        }
        match key.code {
            KeyCode::Esc => return true,
            KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(self.page_height.max(1)),
            KeyCode::PageDown => self.scroll = self.scroll.saturating_add(self.page_height.max(1)),
            KeyCode::Home => self.scroll = 0,
            KeyCode::End => self.scroll = u16::MAX,
            _ => match keys.command(key) {
                Some(Command::Back) => return true,
                Some(command @ (Command::Up | Command::Down)) => {
                    let selected =
                        wrap_selection(self.selected, self.topics.len(), command == Command::Down);
                    if selected != self.selected {
                        self.selected = selected;
                        self.scroll = 0;
                    }
                }
                Some(Command::Left) => {
                    self.scroll = self.scroll.saturating_sub(self.page_height.max(1))
                }
                Some(Command::Right) => {
                    self.scroll = self.scroll.saturating_add(self.page_height.max(1))
                }
                _ => {}
            },
        }
        false
    }

    fn content(&self) -> Vec<Line<'static>> {
        let muted = Style::default().fg(MUTED);
        let note_style = Style::default().fg(FOCUS);
        let speech_style = Style::default().fg(Color::White);
        let Some(topic) = self.topics.get(self.selected) else {
            return vec![Line::styled("No recognized speech.", muted)];
        };
        let mut lines = vec![
            Line::styled(
                format!("Topic {} / {}", self.selected + 1, self.topics.len()),
                muted,
            ),
            Line::default(),
        ];
        for (index, visit) in topic.visits.iter().enumerate() {
            if index > 0 {
                lines.extend([
                    Line::default(),
                    Line::styled("— Later in the discussion —", muted),
                    Line::default(),
                ]);
            }
            let note_text = visit.note.as_ref().map(|note| note.text.as_str());
            if index == 0
                || note_text
                    != topic.visits[index - 1]
                        .note
                        .as_ref()
                        .map(|note| note.text.as_str())
            {
                lines.push(Line::styled(
                    if index == 0 {
                        "ORIGINAL NOTE"
                    } else {
                        "NOTE CHANGED"
                    },
                    note_style.add_modifier(Modifier::BOLD),
                ));
                lines.push(Line::default());
                lines.extend(
                    terminal_text(note_text.unwrap_or("No note presented."))
                        .split('\n')
                        .map(|line| Line::styled(line.to_owned(), note_style)),
                );
                lines.extend([Line::default(), Line::default()]);
            }
            if index == 0 {
                lines.push(Line::styled(
                    "SPOKEN DISCUSSION",
                    speech_style.add_modifier(Modifier::BOLD),
                ));
                lines.push(Line::default());
            }
            if visit.speech.is_empty() {
                lines.push(Line::styled("No recognized speech in this visit.", muted));
            } else {
                for paragraph in &visit.speech {
                    lines.extend(
                        paragraph
                            .split('\n')
                            .map(|line| Line::styled(line.to_owned(), speech_style)),
                    );
                    lines.push(Line::default());
                }
            }
        }
        lines
    }

    pub(crate) fn draw(&mut self, frame: &mut Frame, area: Rect, keys: &Keybindings) {
        let area = area.intersection(frame.area());
        if area.is_empty() {
            return;
        }
        let hint = |command| match keys
            .bindings(command)
            .first()
            .map(String::as_str)
            .unwrap_or("?")
        {
            "Up" => "↑",
            "Down" => "↓",
            "Left" => "←",
            "Right" => "→",
            key => key,
        };
        let controls = Paragraph::new(format!(
            "{}{} topics · {}{}/PgUp/PgDn scroll · Home/End · Esc back · {} help",
            hint(Command::Up),
            hint(Command::Down),
            hint(Command::Left),
            hint(Command::Right),
            hint(Command::Help)
        ))
        .style(Style::default().fg(Color::White).bg(Color::Black));
        let rows = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(area);
        let sidebar_width = if area.width >= 60 {
            (area.width / 3).min(36)
        } else {
            0
        };
        let columns = Layout::horizontal([Constraint::Min(0), Constraint::Length(sidebar_width)])
            .split(rows[0]);
        let inner = solid_panel(
            frame,
            columns[0],
            Block::bordered()
                .padding(Padding::horizontal(u16::from(columns[0].width >= 12)))
                .title(self.title.as_str()),
            FOCUS,
        );
        self.page_height = inner.height;
        let paragraph = Paragraph::new(self.content())
            .style(Style::default().fg(Color::White).bg(Color::Black))
            .wrap(Wrap { trim: false });
        let max_scroll = if inner.is_empty() {
            0
        } else {
            paragraph
                .line_count(inner.width)
                .saturating_sub(usize::from(inner.height))
        };
        self.scroll = self
            .scroll
            .min(u16::try_from(max_scroll).unwrap_or(u16::MAX));
        frame.render_widget(paragraph.scroll((self.scroll, 0)), inner);

        if sidebar_width > 0 {
            let items: Vec<_> = self
                .topics
                .iter()
                .enumerate()
                .map(|(index, topic)| {
                    let excerpt = topic.visits[0].note.as_ref().map_or_else(
                        || "Unassigned discussion".to_owned(),
                        |note| {
                            let text = terminal_text(&note.text)
                                .split_whitespace()
                                .collect::<Vec<_>>()
                                .join(" ");
                            if text.is_empty() {
                                note.kind.label().to_owned()
                            } else {
                                text
                            }
                        },
                    );
                    ListItem::new(format!("{}. {excerpt}", index + 1))
                })
                .collect();
            let inner = solid_panel(frame, columns[1], Block::bordered().title("Topics"), MUTED);
            let mut state = ListState::default()
                .with_selected((!self.topics.is_empty()).then_some(self.selected));
            frame.render_stateful_widget(
                List::new(items)
                    .style(Style::default().fg(MUTED).bg(Color::Black))
                    .scroll_padding(1)
                    .highlight_symbol("▶ ")
                    .highlight_style(Style::default().fg(FOCUS).add_modifier(Modifier::BOLD)),
                inner,
                &mut state,
            );
        }
        frame.render_widget(controls, rows[1]);
    }
}

fn terminal_text(text: &str) -> String {
    text.chars()
        .map(|character| {
            if character.is_control() && character != '\n' {
                ' '
            } else {
                character
            }
        })
        .collect()
}

fn speech_paragraphs(segments: &[&Segment]) -> Vec<String> {
    let mut paragraphs = Vec::new();
    let mut current = String::new();
    let mut previous_end = 0.0_f64;
    for segment in segments {
        let text = terminal_text(&segment.text);
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        // Only add layout: pauses or a few completed sentences separate paragraphs.
        if !current.is_empty()
            && (segment.start - previous_end >= 1.5
                || (current.len() >= 360 && current.ends_with(['.', '!', '?'])))
        {
            paragraphs.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(text);
        previous_end = previous_end.max(segment.end);
    }
    if !current.is_empty() {
        paragraphs.push(current);
    }
    paragraphs
}
