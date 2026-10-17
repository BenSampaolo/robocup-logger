use chrono::{DateTime, FixedOffset, NaiveDate, SecondsFormat};
use serde::{Deserialize, Serialize};

pub(crate) const MARKDOWN_MARKER: &str = "<!-- robocup-logger generated log -->\n";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Metadata {
    pub event: String,
    pub home: String,
    pub away: String,
    pub date: NaiveDate,
    pub label: String,
    pub robots: [String; 5],
}

impl Metadata {
    pub fn hulks_are_home(&self) -> bool {
        self.home.trim().eq_ignore_ascii_case("HULKs")
    }

    pub fn golden_goal_teams(&self) -> (&str, &str) {
        if self.hulks_are_home() {
            (&self.home, "Invisibles")
        } else {
            ("Invisibles", &self.away)
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        for (name, value) in [
            ("Event", &self.event),
            ("Home team", &self.home),
            ("Away team", &self.away),
            ("Label", &self.label),
        ] {
            if value.trim().is_empty() {
                return Err(format!("{name} is required"));
            }
            validate_text(name, value, false)?;
        }
        if self.hulks_are_home() == self.away.trim().eq_ignore_ascii_case("HULKs") {
            return Err("Exactly one team must be HULKs (home or away)".into());
        }
        for robot in &self.robots {
            validate_text("Robot assignment", robot, false)?;
        }
        Ok(())
    }
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum MatchType {
    Official,
    GoldenGoal,
    Combined,
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum EventKind {
    Note,
    GoldenGoalStart,
    GameStart,
    HalfTime,
    SecondHalfStart,
    GoalHome,
    GoalAway,
    GameEnd,
    Custom,
}

impl EventKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Note => "Note",
            Self::GoldenGoalStart => "Golden Goal test",
            Self::GameStart => "Game start",
            Self::HalfTime => "Half-time",
            Self::SecondHalfStart => "Second half start",
            Self::GoalHome => "Goal home",
            Self::GoalAway => "Goal away",
            Self::GameEnd => "Game end",
            Self::Custom => "Custom",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture_id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture_after: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub game: Option<MatchType>,
    pub timestamp: DateTime<FixedOffset>,
    #[serde(default)]
    pub gc_seconds_remaining: Option<i16>,
    pub kind: EventKind,
    pub text: String,
    #[serde(default)]
    pub undone: bool,
    #[serde(default)]
    pub from_controller: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_budget: Option<u16>,
    // A transition packet can reset the budget; the test summary uses the preceding reading.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub golden_goal_message_budget: Option<u16>,
    // Controller sync/correction deltas adjust this game's score (official score in legacy logs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score_delta: Option<[i32; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub statistics: Option<crate::statistics::Statistics>,
}

impl Entry {
    fn has_controller_snapshot(&self) -> bool {
        if !self.from_controller
            || self.kind == EventKind::Note
            || !(self.text.starts_with("GC received: ")
                || self.text.starts_with("GC sync — received: "))
        {
            return false;
        }
        // Match the complete generated suffix, not just the prefix: appended human
        // annotations must remain visible, including in sessions saved by older versions.
        let Some((_, clock)) = self.text.rsplit_once("; secondary ") else {
            return false;
        };
        let clock = clock
            .strip_suffix("; phase observed on attachment, not the original transition time")
            .unwrap_or(clock);
        let Some((minutes, seconds)) = clock.split_once(':') else {
            return false;
        };
        minutes.parse::<i16>().is_ok()
            && seconds.len() == 2
            && seconds.parse::<u8>().is_ok_and(|seconds| seconds < 60)
    }

    pub(crate) fn ends_golden_goal(&self) -> bool {
        matches!(self.kind, EventKind::GameStart | EventKind::SecondHalfStart)
            || (self.from_controller
                && matches!(self.kind, EventKind::HalfTime | EventKind::GameEnd))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Session {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture_session: Option<String>,
    pub metadata: Metadata,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub match_type: Option<MatchType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub golden_goal_opponent: Option<u8>,
    pub entries: Vec<Entry>,
    pub active_note: Option<usize>,
}

impl Session {
    pub fn new(metadata: Metadata) -> Self {
        Self {
            capture_session: None,
            metadata,
            match_type: None,
            golden_goal_opponent: None,
            entries: Vec::new(),
            active_note: None,
        }
    }

    pub fn new_match(metadata: Metadata, match_type: MatchType) -> Self {
        Self {
            match_type: Some(match_type),
            ..Self::new(metadata)
        }
    }

    pub fn current_game(&self) -> MatchType {
        match self.match_type {
            Some(MatchType::Combined) => self
                .entries
                .iter()
                .rev()
                .filter(|entry| !entry.undone)
                .find_map(|entry| match entry.game {
                    Some(game @ (MatchType::GoldenGoal | MatchType::Official)) => Some(game),
                    _ => None,
                })
                .unwrap_or(MatchType::GoldenGoal),
            Some(game) => game,
            None if self.in_golden_goal() => MatchType::GoldenGoal,
            None => MatchType::Official,
        }
    }

    pub fn is_finished(&self) -> bool {
        if self.match_type == Some(MatchType::Combined) {
            return self.is_game_finished(self.current_game());
        }
        self.entries
            .iter()
            .any(|entry| !entry.undone && entry.kind == EventKind::GameEnd)
    }

    pub fn is_game_finished(&self, game: MatchType) -> bool {
        if self.match_type != Some(MatchType::Combined) {
            return game == self.current_game() && self.is_finished();
        }
        game != MatchType::Combined
            && self.entries.iter().any(|entry| {
                !entry.undone && entry.game == Some(game) && entry.kind == EventKind::GameEnd
            })
    }

    pub fn start_note(&mut self, now: DateTime<FixedOffset>) {
        if self.active_note.is_none() {
            self.active_note = Some(self.entries.len());
            self.record(EventKind::Note, now);
        }
    }

    pub fn submit_note(&mut self) {
        self.active_note = None;
    }

    pub fn record(&mut self, kind: EventKind, now: DateTime<FixedOffset>) {
        self.entries.push(Entry {
            capture_id: None,
            capture_after: None,
            game: (self.match_type == Some(MatchType::Combined)).then(|| self.current_game()),
            timestamp: now,
            gc_seconds_remaining: None,
            kind,
            text: String::new(),
            undone: false,
            from_controller: false,
            message_budget: None,
            golden_goal_message_budget: None,
            score_delta: None,
            statistics: None,
        });
    }

    pub fn delete(&mut self, index: usize) {
        if let Some(entry) = self.entries.get_mut(index) {
            entry.undone = true;
            if self.active_note == Some(index) {
                self.active_note = None;
            }
        }
    }

    pub fn restore(&mut self, index: usize) {
        if let Some(entry) = self.entries.get_mut(index) {
            entry.undone = false;
        }
    }

    pub fn score(&self) -> (usize, usize) {
        let (home, away) = self.raw_score();
        (home.max(0) as usize, away.max(0) as usize)
    }

    pub fn raw_score_for(&self, game: MatchType) -> (i64, i64) {
        if game == MatchType::Combined {
            return (0, 0);
        }
        if self.match_type != Some(MatchType::Combined) {
            return match (self.match_type, game) {
                (Some(fixed), game) if fixed == game => self.raw_score(),
                (None, MatchType::Official) => self.raw_score(),
                (None, MatchType::GoldenGoal) => {
                    let (home, away) = self.golden_goal_score();
                    (home as i64, away as i64)
                }
                _ => (0, 0),
            };
        }
        let (mut home, mut away) = (0_i64, 0_i64);
        for entry in self
            .entries
            .iter()
            .filter(|entry| !entry.undone && entry.game == Some(game))
        {
            if let Some([home_delta, away_delta]) = entry.score_delta {
                home += i64::from(home_delta);
                away += i64::from(away_delta);
            }
            match entry.kind {
                EventKind::GoalHome => home += 1,
                EventKind::GoalAway => away += 1,
                _ => {}
            }
        }
        (home, away)
    }

    pub fn raw_score(&self) -> (i64, i64) {
        if self.match_type == Some(MatchType::Combined) {
            return self.raw_score_for(MatchType::Official);
        }
        let (mut home, mut away) = (0_i64, 0_i64);
        let mut in_golden_goal = false;
        for entry in self.entries.iter().filter(|entry| !entry.undone) {
            if let Some([home_delta, away_delta]) = entry.score_delta {
                home += i64::from(home_delta);
                away += i64::from(away_delta);
            }
            match entry.kind {
                EventKind::GoldenGoalStart => in_golden_goal = true,
                _ if entry.ends_golden_goal() => in_golden_goal = false,
                EventKind::GoalHome
                    if self.match_type.is_some() || !in_golden_goal || entry.from_controller =>
                {
                    home += 1
                }
                EventKind::GoalAway
                    if self.match_type.is_some() || !in_golden_goal || entry.from_controller =>
                {
                    away += 1
                }
                _ => {}
            }
        }
        (home, away)
    }

    pub fn in_golden_goal(&self) -> bool {
        if self.match_type == Some(MatchType::Combined) {
            return self.current_game() == MatchType::GoldenGoal;
        }
        if let Some(match_type) = self.match_type {
            return match_type == MatchType::GoldenGoal;
        }
        self.entries
            .iter()
            .rev()
            .filter(|entry| !entry.undone)
            .find_map(|entry| match entry.kind {
                EventKind::GoldenGoalStart => Some(true),
                _ if entry.ends_golden_goal() => Some(false),
                _ => None,
            })
            .unwrap_or(false)
    }

    pub fn current_teams(&self) -> (&str, &str) {
        if self.in_golden_goal() {
            self.metadata.golden_goal_teams()
        } else {
            (&self.metadata.home, &self.metadata.away)
        }
    }

    pub fn golden_goal_score(&self) -> (usize, usize) {
        match self.match_type {
            Some(MatchType::GoldenGoal) => return self.score(),
            Some(MatchType::Official) => return (0, 0),
            Some(MatchType::Combined) => {
                let (home, away) = self.raw_score_for(MatchType::GoldenGoal);
                return (home.max(0) as usize, away.max(0) as usize);
            }
            None => {}
        }
        let (mut home, mut away) = (0, 0);
        let mut in_golden_goal = false;
        for entry in self.entries.iter().filter(|entry| !entry.undone) {
            match entry.kind {
                EventKind::GoldenGoalStart => in_golden_goal = true,
                _ if entry.ends_golden_goal() => in_golden_goal = false,
                EventKind::GoalHome if in_golden_goal && !entry.from_controller => home += 1,
                EventKind::GoalAway if in_golden_goal && !entry.from_controller => away += 1,
                _ => {}
            }
        }
        (home, away)
    }

    pub fn validate(&self) -> Result<(), String> {
        self.metadata.validate()?;
        if self.match_type == Some(MatchType::GoldenGoal) {
            let opponent = if self.metadata.hulks_are_home() {
                &self.metadata.away
            } else {
                &self.metadata.home
            };
            if !opponent.trim().eq_ignore_ascii_case("Invisibles") {
                return Err("Golden Goal opponent must be Invisibles".into());
            }
        }
        if let Some(index) = self.active_note {
            match self.entries.get(index) {
                Some(entry) if entry.kind == EventKind::Note && !entry.undone => {}
                _ => return Err("Active note must refer to an existing note".into()),
            }
        }
        for entry in &self.entries {
            validate_text("Entry text", &entry.text, true)?;
            if entry.game == Some(MatchType::Combined)
                || (self.match_type == Some(MatchType::Combined) && entry.game.is_none())
            {
                return Err("Combined entries must belong to Golden Goal or Official".into());
            }
            let game = if self.match_type == Some(MatchType::Combined) {
                entry.game
            } else {
                self.match_type
            };
            if let Some(statistics) = &entry.statistics {
                if game == Some(MatchType::GoldenGoal)
                    || entry.game == Some(MatchType::GoldenGoal)
                    || entry.kind == EventKind::GoldenGoalStart
                {
                    return Err("Statistics must belong to an official match".into());
                }
                if statistics
                    .penalties
                    .iter()
                    .any(|change| !(1..=20).contains(&change.player))
                {
                    return Err("Statistics player number must be between 1 and 20".into());
                }
            }
            match (game, entry.kind) {
                (Some(MatchType::Official), EventKind::GoldenGoalStart) => {
                    return Err("Official games cannot contain a Golden Goal start".into());
                }
                (
                    Some(MatchType::GoldenGoal),
                    EventKind::GameStart | EventKind::HalfTime | EventKind::SecondHalfStart,
                ) => return Err("Golden Goal games cannot contain official-half events".into()),
                _ => {}
            }
        }
        Ok(())
    }

    pub fn markdown(&self) -> String {
        let fixed_golden_goal = self.match_type == Some(MatchType::GoldenGoal);
        let (home, away) = if fixed_golden_goal {
            self.current_teams()
        } else {
            (self.metadata.home.as_str(), self.metadata.away.as_str())
        };
        let home = markdown_text(home);
        let away = markdown_text(away);
        let mut output = format!(
            "# Log {} {} {} vs {}\n\n## Robot Assignments\n",
            self.metadata.date.format("%d.%m.%Y"),
            markdown_text(&self.metadata.event),
            home,
            away,
        );
        for (index, robot) in self.metadata.robots.iter().enumerate() {
            output.push_str(&format!("- {}: {}\n", index + 1, markdown_text(robot)));
        }
        output.push('\n');
        if self.match_type == Some(MatchType::Combined) {
            self.markdown_entries(&mut output, Some(MatchType::GoldenGoal));
            self.markdown_entries(&mut output, Some(MatchType::Official));
        } else {
            self.markdown_entries(&mut output, self.match_type);
        }
        output.push('\n');
        output.push_str(MARKDOWN_MARKER);
        output
    }

    fn markdown_entries(&self, output: &mut String, game: Option<MatchType>) {
        let fixed_golden_goal = game == Some(MatchType::GoldenGoal);
        let (home, away) = if fixed_golden_goal {
            self.metadata.golden_goal_teams()
        } else {
            (self.metadata.home.as_str(), self.metadata.away.as_str())
        };
        let home = markdown_text(home);
        let away = markdown_text(away);
        let (test_home, test_away) = self.metadata.golden_goal_teams();
        let test_home = markdown_text(test_home);
        let test_away = markdown_text(test_away);
        if fixed_golden_goal {
            output.push_str("## Golden Goal\n\n");
        }
        let (mut home_score, mut away_score) = (0_i64, 0_i64);
        let (mut test_home_score, mut test_away_score) = (0_i64, 0_i64);
        let mut in_golden_goal = false;
        let mut needs_official_heading =
            self.match_type == Some(MatchType::Combined) && game == Some(MatchType::Official);
        for entry in self.entries.iter().filter(|entry| {
            !entry.undone && (self.match_type != Some(MatchType::Combined) || entry.game == game)
        }) {
            let gc_snapshot = entry.has_controller_snapshot();
            if gc_snapshot
                && entry.kind == EventKind::Custom
                && entry.score_delta.is_none_or(|delta| delta == [0, 0])
            {
                continue;
            }
            let gc_sync = gc_snapshot && entry.text.starts_with("GC sync — received: ");
            if let Some([home_delta, away_delta]) = entry.score_delta {
                home_score += i64::from(home_delta);
                away_score += i64::from(away_delta);
            }
            match entry.kind {
                EventKind::GoldenGoalStart if game.is_none() => in_golden_goal = true,
                _ if in_golden_goal && entry.ends_golden_goal() => {
                    output.push_str(&format!(
                        "\nGolden Goal score: {test_home} {test_home_score}:{test_away_score} {test_away}\n\n"
                    ));
                    output.push_str(&message_budget_summary(entry.golden_goal_message_budget));
                    in_golden_goal = false;
                }
                _ => {}
            }
            let heading = match entry.kind {
                _ if fixed_golden_goal => None,
                EventKind::GoldenGoalStart => Some("Golden Goal"),
                EventKind::GameStart => Some("1. Halbzeit"),
                EventKind::HalfTime => Some("Halbzeit"),
                EventKind::SecondHalfStart => Some("2. Halbzeit"),
                _ => None,
            };
            if let Some(heading) =
                heading.or_else(|| needs_official_heading.then_some("Official match"))
            {
                output.push_str(&format!("\n## {heading}\n\n"));
                needs_official_heading = false;
            }
            let timestamp = if game.is_some() {
                entry.gc_seconds_remaining.map_or_else(
                    || "GC time unavailable".into(),
                    |seconds| {
                        let sign = if seconds < 0 { "-" } else { "" };
                        let seconds = seconds.unsigned_abs();
                        format!("{sign}{:02}:{:02}", seconds / 60, seconds % 60)
                    },
                )
            } else {
                // Legacy logs retain their original wall-clock representation.
                entry
                    .timestamp
                    .to_rfc3339_opts(SecondsFormat::Millis, false)
            };
            let text = if gc_snapshot {
                String::new()
            } else {
                markdown_text(&entry.text).replace('\n', "\n  ")
            };
            let line = match entry.kind {
                EventKind::Note => text.clone(),
                EventKind::Custom if entry.score_delta.is_some() => if gc_sync {
                    "Score sync"
                } else {
                    "Score correction"
                }
                .to_string(),
                EventKind::GoalHome | EventKind::GoalAway => {
                    let (home_score, away_score, home, away) =
                        if in_golden_goal && !entry.from_controller {
                            (
                                &mut test_home_score,
                                &mut test_away_score,
                                test_home.as_str(),
                                test_away.as_str(),
                            )
                        } else {
                            (
                                &mut home_score,
                                &mut away_score,
                                home.as_str(),
                                away.as_str(),
                            )
                        };
                    let team = if entry.kind == EventKind::GoalHome {
                        *home_score += 1;
                        home
                    } else {
                        *away_score += 1;
                        away
                    };
                    format!(
                        "Goal {team} — {home} {}:{} {away}",
                        (*home_score).max(0),
                        (*away_score).max(0)
                    )
                }
                _ => entry.kind.label().to_string(),
            };
            let goal = matches!(entry.kind, EventKind::GoalHome | EventKind::GoalAway);
            if goal {
                output.push_str(&format!("\n{line} [{timestamp}]"));
            } else {
                output.push_str(&format!("- [{timestamp}] {line}"));
            }
            if entry.kind != EventKind::Note && !text.is_empty() {
                output.push_str(&format!(" — {text}"));
            }
            if gc_sync && entry.kind != EventKind::Custom {
                output.push_str(" — GC sync (original event time unknown)");
            }
            if let Some([home_delta, away_delta]) = entry.score_delta {
                output.push_str(&format!(
                    " — {home} {}:{} {away} ({home_delta:+}:{away_delta:+})",
                    home_score.max(0),
                    away_score.max(0)
                ));
            }
            output.push('\n');
            if goal {
                output.push('\n');
            }
            if matches!(entry.kind, EventKind::HalfTime | EventKind::GameEnd) {
                let summary = if fixed_golden_goal {
                    "Golden Goal final score"
                } else if entry.kind == EventKind::HalfTime {
                    "Half-time score"
                } else {
                    "Final score"
                };
                // An end marker can be removed, followed by more goals, then restored.
                let (summary_home, summary_away) =
                    if let Some(game) = game.filter(|_| entry.kind == EventKind::GameEnd) {
                        let (home, away) = self.raw_score_for(game);
                        (home.max(0) as usize, away.max(0) as usize)
                    } else {
                        (home_score.max(0) as usize, away_score.max(0) as usize)
                    };
                output.push_str(&format!(
                    "\n{summary}: {home} {summary_home}:{summary_away} {away}\n\n"
                ));
                output.push_str(&message_budget_summary(entry.message_budget));
            }
        }
        if !fixed_golden_goal && let Some(summary) = crate::statistics::markdown(self) {
            output.push_str(&summary);
        }
        if in_golden_goal {
            output.push_str(&format!(
                "\nGolden Goal score: {test_home} {test_home_score}:{test_away_score} {test_away}\n\n"
            ));
        } else if fixed_golden_goal && !self.is_game_finished(MatchType::GoldenGoal) {
            output.push_str(&format!(
                "\nGolden Goal score: {home} {}:{} {away}\n\n",
                home_score.max(0),
                away_score.max(0)
            ));
        }
    }
}

fn message_budget_summary(budget: Option<u16>) -> String {
    format!(
        "HULKs remaining message budget: {}\n\n",
        budget.map_or_else(
            || "unavailable (no fresh GC reading)".into(),
            |value| value.to_string()
        )
    )
}

fn validate_text(name: &str, value: &str, multiline: bool) -> Result<(), String> {
    if value.chars().any(|c| {
        (c.is_control() && !(multiline && c == '\n')) || matches!(c, '\u{2028}' | '\u{2029}')
    }) {
        return Err(format!(
            "{name} contains unsupported control characters or line breaks"
        ));
    }
    Ok(())
}

pub(crate) fn markdown_text(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        if matches!(
            character,
            '\\' | '`' | '*' | '_' | '[' | ']' | '<' | '>' | '#' | '!'
        ) {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata() -> Metadata {
        Metadata {
            event: "hobbymesse".into(),
            home: "HULKs".into(),
            away: "HTWK".into(),
            date: NaiveDate::from_ymd_opt(2026, 10, 3).unwrap(),
            label: "1st_game".into(),
            robots: ["42".into(), "44".into(), "43".into(), "".into(), "".into()],
        }
    }

    fn time(value: &str) -> DateTime<FixedOffset> {
        DateTime::parse_from_rfc3339(value).unwrap()
    }

    const GC_SNAPSHOT: &str = "GC received: HULKs 0:0 HTWK; opponent ID 5; Normal / Playing / first half; set play: None; kicking team: none; stopped: no; clock 09:50; secondary 00:00";

    #[test]
    fn minimal_markdown_preserves_scores_budgets_timestamps_and_raw_json() {
        let mut session = Session::new(metadata());
        let timestamp = time("2026-10-03T12:34:56.789+02:00");
        for (old, new) in [
            ("Playing", "Ready"),
            ("Playing", "Set"),
            ("Playing", "Playing"),
            ("set play: None", "set play: Throw-in"),
            ("stopped: no", "stopped: yes"),
        ] {
            session.record(EventKind::Custom, timestamp);
            let entry = session.entries.last_mut().unwrap();
            entry.from_controller = true;
            entry.text = GC_SNAPSHOT.replace(old, new);
        }
        for kind in [
            EventKind::GameStart,
            EventKind::GoalHome,
            EventKind::GoalAway,
            EventKind::Custom,
            EventKind::HalfTime,
        ] {
            session.record(kind, timestamp);
            let entry = session.entries.last_mut().unwrap();
            entry.from_controller = true;
            entry.text = GC_SNAPSHOT.into();
            if kind == EventKind::Custom {
                entry.score_delta = Some([-1, 0]);
            }
            if kind == EventKind::HalfTime {
                entry.message_budget = Some(12000);
            }
        }
        session.record(EventKind::Note, timestamp);
        session.entries.last_mut().unwrap().text = "Keeper lost the ball".into();
        session.record(EventKind::Custom, timestamp);
        session.entries.last_mut().unwrap().text = "Battery swap".into();
        let saved = serde_json::to_string(&session).unwrap();
        let markdown = session.markdown();
        assert!(markdown.contains("Goal HULKs — HULKs 1:0 HTWK"));
        assert!(markdown.contains("Goal HTWK — HULKs 1:1 HTWK"));
        assert!(markdown.contains("Score correction — HULKs 0:1 HTWK (-1:+0)"));
        assert!(markdown.contains("Half-time score: HULKs 0:1 HTWK"));
        assert!(markdown.contains("HULKs remaining message budget: 12000"));
        assert!(markdown.contains("Keeper lost the ball"));
        assert!(markdown.contains("Battery swap"));
        assert_eq!(
            markdown.matches("[2026-10-03T12:34:56.789+02:00]").count(),
            7
        );
        for diagnostic in [
            "GC received",
            "opponent ID",
            "clock",
            "Ready",
            "Set",
            "Throw-in",
            "stopped",
        ] {
            assert!(
                !markdown.contains(diagnostic),
                "{diagnostic} leaked into Markdown"
            );
        }
        assert_eq!(serde_json::to_string(&session).unwrap(), saved);
        assert!(saved.contains(GC_SNAPSHOT));
        assert_eq!(
            serde_json::from_str::<Session>(&saved).unwrap().markdown(),
            markdown
        );
        let correction = session
            .entries
            .iter()
            .position(|entry| entry.score_delta.is_some())
            .unwrap();
        session.delete(correction);
        assert_eq!(session.score(), (1, 1));
        assert!(!session.markdown().contains("Score correction"));
        session.restore(correction);
        assert_eq!(session.markdown(), markdown);
    }

    #[test]
    fn minimal_markdown_labels_syncs_without_inventing_goals_or_phase_times() {
        for kind in [EventKind::Custom, EventKind::GameStart] {
            let mut session = Session::new(metadata());
            session.record(kind, time("2026-10-03T12:00:00+02:00"));
            let entry = &mut session.entries[0];
            entry.from_controller = true;
            entry.score_delta = Some([2, 1]);
            entry.text = GC_SNAPSHOT.replace("GC received:", "GC sync — received:");
            if kind == EventKind::GameStart {
                entry
                    .text
                    .push_str("; phase observed on attachment, not the original transition time");
            }
            let markdown = session.markdown();
            assert!(markdown.contains("HULKs 2:1 HTWK (+2:+1)"));
            assert!(!markdown.contains("Goal HULKs"));
            assert!(!markdown.contains("opponent ID"));
            if kind == EventKind::Custom {
                assert!(markdown.contains("Score sync"));
            } else {
                assert!(markdown.contains("Game start — GC sync (original event time unknown)"));
            }
        }
    }

    #[test]
    fn minimal_markdown_preserves_manual_notes_and_edited_controller_annotations() {
        for (kind, from_controller, text) in [
            (EventKind::Note, true, GC_SNAPSHOT.to_string()),
            (EventKind::Custom, false, GC_SNAPSHOT.to_string()),
            (EventKind::Custom, true, "Robot fell".into()),
            (
                EventKind::GoalHome,
                true,
                format!("{GC_SNAPSHOT}; keeper out of position"),
            ),
            (
                EventKind::Custom,
                true,
                format!("{GC_SNAPSHOT}\nRobot fell"),
            ),
        ] {
            let mut session = Session::new(metadata());
            session.record(kind, time("2026-10-03T12:00:00+02:00"));
            let entry = &mut session.entries[0];
            entry.from_controller = from_controller;
            entry.text = text.clone();
            assert!(session.markdown().contains(&text.replace('\n', "\n  ")));
        }
    }

    #[test]
    fn controller_snapshot_detection_handles_signed_and_long_clocks() {
        let mut session = Session::new(metadata());
        session.record(EventKind::Custom, time("2026-10-03T12:00:00+02:00"));
        let entry = &mut session.entries[0];
        entry.from_controller = true;
        for clock in ["00:00", "-00:01", "546:07", "-546:08"] {
            entry.text = GC_SNAPSHOT.replace("secondary 00:00", &format!("secondary {clock}"));
            assert!(entry.has_controller_snapshot());
        }
    }

    #[test]
    fn metadata_requires_exactly_one_hulks_team_and_preserves_its_side() {
        for (home, away, hulks_are_home) in [
            ("HULKs", "HTWK", true),
            (" hulks ", "HTWK", true),
            ("HTWK", "HULKs", false),
            ("HTWK", " hUlKs ", false),
        ] {
            let mut metadata = metadata();
            metadata.home = home.into();
            metadata.away = away.into();
            assert!(metadata.validate().is_ok());
            assert_eq!(metadata.hulks_are_home(), hulks_are_home);
            assert_eq!(
                metadata.golden_goal_teams(),
                if hulks_are_home {
                    (home, "Invisibles")
                } else {
                    ("Invisibles", away)
                }
            );
        }
        for (home, away) in [("HTWK", "Bembelbots"), ("HULKs", " hulks ")] {
            let mut metadata = metadata();
            metadata.home = home.into();
            metadata.away = away.into();
            assert_eq!(
                metadata.validate().unwrap_err(),
                "Exactly one team must be HULKs (home or away)"
            );
        }
    }

    #[test]
    fn draft_keeps_first_timestamp_through_events_editing_and_submission() {
        let mut session = Session::new(metadata());
        let first = time("2026-10-03T23:59:59.123+02:00");
        let later = time("2026-10-04T00:00:01.456+02:00");
        session.start_note(first);
        session.entries[0].text = "first\nsecond".into();
        session.record(EventKind::GoalHome, later);
        session.record(EventKind::HalfTime, later);
        session.start_note(later);
        assert_eq!(session.active_note, Some(0));
        assert_eq!(session.entries.len(), 3);
        session.entries[0].text.push_str(" corrected");
        session.submit_note();
        assert_eq!(session.entries[0].timestamp, first);
        assert!(session.markdown().contains("2026-10-03T23:59:59.123+02:00"));
        assert!(session.markdown().contains("2026-10-04T00:00:01.456+02:00"));
        assert!(session.markdown().contains("first\n  second corrected"));
        session.start_note(later);
        assert_eq!(session.active_note, Some(3));
    }

    #[test]
    fn corrected_events_rebuild_phase_headings_and_cumulative_scores() {
        let mut session = Session::new(metadata());
        let now = time("2026-10-03T12:00:00.000+02:00");
        for kind in [
            EventKind::GameStart,
            EventKind::GoalHome,
            EventKind::HalfTime,
            EventKind::SecondHalfStart,
            EventKind::GoalAway,
            EventKind::GameEnd,
        ] {
            session.record(kind, now);
        }
        let original = session.markdown();
        assert!(original.starts_with("# Log 03.10.2026 hobbymesse HULKs vs HTWK"));
        for heading in ["## 1. Halbzeit", "## Halbzeit", "## 2. Halbzeit"] {
            assert!(original.contains(heading));
        }
        assert!(original.contains("Half-time score: HULKs 1:0 HTWK"));
        assert!(original.contains("Final score: HULKs 1:1 HTWK"));
        session.entries[1].kind = EventKind::GoalAway;
        session.delete(4);
        assert_eq!(session.score(), (0, 1));
        let corrected = session.markdown();
        assert!(
            corrected.contains("\n\nGoal HTWK — HULKs 0:1 HTWK [2026-10-03T12:00:00.000+02:00]")
        );
        assert!(corrected.contains("Half-time score: HULKs 0:1 HTWK"));
        assert!(corrected.contains("Final score: HULKs 0:1 HTWK"));
        assert!(!corrected.contains("Goal HULKs"));
        assert_eq!(corrected, session.markdown());
    }

    #[test]
    fn golden_goal_scores_are_separate_until_either_match_start() {
        let now = time("2026-10-03T12:00:00+02:00");
        let later = time("2026-10-03T12:01:00+02:00");
        for start in [EventKind::GameStart, EventKind::SecondHalfStart] {
            let mut session = Session::new(metadata());
            assert!(!session.in_golden_goal());
            session.record(EventKind::GoalHome, now);
            session.record(EventKind::GoldenGoalStart, later);
            // Routing follows entry order, even if the clock moves backwards.
            for kind in [
                EventKind::GoalHome,
                EventKind::HalfTime,
                EventKind::GoalAway,
                EventKind::GameEnd,
                EventKind::Custom,
                EventKind::Note,
                EventKind::GoldenGoalStart,
                EventKind::GoalHome,
            ] {
                session.record(kind, now);
            }
            assert!(session.in_golden_goal());
            assert_eq!(session.score(), (1, 0));
            assert_eq!(session.golden_goal_score(), (2, 1));
            let preview = session.markdown();
            assert!(preview.contains("Half-time score: HULKs 1:0 HTWK"));
            assert!(preview.contains("Final score: HULKs 1:0 HTWK"));
            assert!(preview.contains("Golden Goal score: HULKs 2:1 Invisibles"));

            session.record(start, now);
            session.record(EventKind::GoalAway, now);
            assert!(!session.in_golden_goal());
            assert_eq!(session.score(), (1, 1));
            assert_eq!(session.golden_goal_score(), (2, 1));
            let markdown = session.markdown();
            let heading = if start == EventKind::GameStart {
                "## 1. Halbzeit"
            } else {
                "## 2. Halbzeit"
            };
            assert!(
                markdown
                    .find("Golden Goal score: HULKs 2:1 Invisibles")
                    .unwrap()
                    < markdown.find(heading).unwrap()
            );
        }
    }

    #[test]
    fn golden_goal_markdown_keeps_invisibles_test_and_official_match_separate() {
        let mut session = Session::new(metadata());
        let now = time("2026-10-03T12:00:00.123+02:00");
        session.record(EventKind::GoldenGoalStart, now);
        session.entries[0].text = "deployment ready".into();
        session.start_note(now);
        session.entries[1].text = "robot check".into();
        session.record(EventKind::GoalHome, now);
        session.record(EventKind::GoalAway, now);
        let preview = session.markdown();
        assert!(preview.starts_with("# Log 03.10.2026 hobbymesse HULKs vs HTWK"));
        assert!(preview.contains("## Golden Goal\n"));
        assert!(
            preview
                .contains("- [2026-10-03T12:00:00.123+02:00] Golden Goal test — deployment ready")
        );
        assert!(preview.contains("- [2026-10-03T12:00:00.123+02:00] robot check"));
        assert!(preview.contains("Goal HULKs — HULKs 1:0 Invisibles"));
        assert!(preview.contains("Goal Invisibles — HULKs 1:1 Invisibles"));
        assert!(preview.contains("Golden Goal score: HULKs 1:1 Invisibles"));
        assert_eq!(session.current_teams(), ("HULKs", "Invisibles"));
        assert_eq!(session.score(), (0, 0));
        assert_eq!(session.active_note, Some(1));
        assert_eq!(session.entries[1].timestamp, now);

        for kind in [
            EventKind::GameStart,
            EventKind::GoalAway,
            EventKind::HalfTime,
            EventKind::SecondHalfStart,
            EventKind::GoalHome,
            EventKind::GameEnd,
        ] {
            session.record(kind, now);
        }
        let markdown = session.markdown();
        assert!(markdown.contains("## 1. Halbzeit\n"));
        assert_eq!(session.current_teams(), ("HULKs", "HTWK"));
        assert_eq!(markdown.matches("Golden Goal score:").count(), 1);
        assert!(markdown.contains("Goal HTWK — HULKs 0:1 HTWK"));
        assert!(markdown.contains("Half-time score: HULKs 0:1 HTWK"));
        assert!(markdown.contains("Final score: HULKs 1:1 HTWK"));
        assert!(!markdown.contains("Final score: HULKs 2:2 HTWK"));
    }

    #[test]
    fn golden_goal_preserves_home_away_scores_when_hulks_are_away() {
        let mut metadata = metadata();
        metadata.home = "HTWK".into();
        metadata.away = "HULKs".into();
        let mut session = Session::new(metadata);
        let now = time("2026-10-03T12:00:00+02:00");
        assert_eq!(session.current_teams(), ("HTWK", "HULKs"));
        session.record(EventKind::GoldenGoalStart, now);
        session.record(EventKind::GoalHome, now);
        session.record(EventKind::GoalHome, now);
        session.record(EventKind::GoalAway, now);
        assert_eq!(session.current_teams(), ("Invisibles", "HULKs"));
        assert_eq!(session.golden_goal_score(), (2, 1));
        assert_eq!(session.score(), (0, 0));
        let preview = session.markdown();
        assert!(preview.starts_with("# Log 03.10.2026 hobbymesse HTWK vs HULKs"));
        assert!(preview.contains("Goal Invisibles — Invisibles 1:0 HULKs"));
        assert!(preview.contains("Goal HULKs — Invisibles 2:1 HULKs"));
        assert!(preview.contains("Golden Goal score: Invisibles 2:1 HULKs"));

        session.record(EventKind::GameStart, now);
        session.record(EventKind::GoalAway, now);
        session.record(EventKind::GoalAway, now);
        session.record(EventKind::GoalHome, now);
        session.record(EventKind::GameEnd, now);
        assert_eq!(session.current_teams(), ("HTWK", "HULKs"));
        assert_eq!(session.golden_goal_score(), (2, 1));
        assert_eq!(session.score(), (1, 2));
        let markdown = session.markdown();
        assert!(markdown.contains("Golden Goal score: Invisibles 2:1 HULKs"));
        assert!(markdown.contains("Goal HULKs — HTWK 0:1 HULKs"));
        assert!(markdown.contains("Goal HTWK — HTWK 1:2 HULKs"));
        assert!(markdown.contains("Final score: HTWK 1:2 HULKs"));
    }

    #[test]
    fn correcting_and_deleting_test_goals_only_changes_test_score() {
        let mut session = Session::new(metadata());
        let now = time("2026-10-03T12:00:00+02:00");
        for kind in [
            EventKind::GameStart,
            EventKind::GoalHome,
            EventKind::GoldenGoalStart,
            EventKind::GoalHome,
            EventKind::GoalAway,
            EventKind::GameStart,
            EventKind::GoalAway,
        ] {
            session.record(kind, now);
        }
        assert_eq!(session.score(), (1, 1));
        assert_eq!(session.golden_goal_score(), (1, 1));
        session.entries[3].kind = EventKind::GoalAway;
        assert_eq!(session.score(), (1, 1));
        assert_eq!(session.golden_goal_score(), (0, 2));
        session.delete(4);
        assert_eq!(session.score(), (1, 1));
        assert_eq!(session.golden_goal_score(), (0, 1));
        assert!(
            session
                .markdown()
                .contains("Golden Goal score: HULKs 0:1 Invisibles")
        );
    }

    #[test]
    fn correcting_and_deleting_markers_rebuilds_goal_routing() {
        let mut session = Session::new(metadata());
        let now = time("2026-10-03T12:00:00+02:00");
        for kind in [
            EventKind::GoldenGoalStart,
            EventKind::GoalHome,
            EventKind::GameStart,
            EventKind::GoalAway,
        ] {
            session.record(kind, now);
        }
        assert_eq!(session.score(), (0, 1));
        assert_eq!(session.golden_goal_score(), (1, 0));
        session.entries[2].kind = EventKind::Custom;
        assert!(session.in_golden_goal());
        assert_eq!(session.score(), (0, 0));
        assert_eq!(session.golden_goal_score(), (1, 1));
        session.delete(0);
        assert!(!session.in_golden_goal());
        assert_eq!(session.score(), (1, 1));
        assert_eq!(session.golden_goal_score(), (0, 0));
        assert!(!session.markdown().contains("Golden Goal"));
    }

    #[test]
    fn old_sessions_default_new_fields_and_golden_goal_markers_roundtrip() {
        let old = serde_json::json!({
            "metadata": {
                "event": "hobbymesse", "home": "HULKs", "away": "HTWK",
                "date": "2026-10-03", "label": "1st_game",
                "robots": ["42", "44", "43", "", ""]
            },
            "entries": [{
                "timestamp": "2026-10-03T12:00:00+02:00",
                "kind": "GoalHome", "text": "legacy goal"
            }],
            "active_note": null
        });
        let mut session: Session = serde_json::from_value(old.clone()).unwrap();
        assert_eq!(session.score(), (1, 0));
        assert_eq!(session.golden_goal_score(), (0, 0));
        assert!(!session.in_golden_goal());
        assert!(!session.entries[0].undone);
        assert_eq!(session.entries[0].score_delta, None);
        assert_eq!(session.entries[0].gc_seconds_remaining, None);
        assert_eq!(session.match_type, None);
        assert!(
            session
                .markdown()
                .contains("[2026-10-03T12:00:00.000+02:00]")
        );

        let now = time("2026-10-03T12:01:00+02:00");
        session.record(EventKind::GoldenGoalStart, now);
        session.record(EventKind::GoalAway, now);
        let encoded = serde_json::to_value(&session).unwrap();
        assert_eq!(encoded["entries"][1]["kind"], "GoldenGoalStart");
        assert_eq!(encoded.as_object().unwrap().len(), 3);
        let restored: Session = serde_json::from_value(encoded).unwrap();
        assert_eq!(restored, session);
        assert_eq!(restored.score(), (1, 0));
        assert_eq!(restored.golden_goal_score(), (0, 1));
        assert!(restored.in_golden_goal());
    }

    #[test]
    fn deleting_entries_tracks_the_active_note() {
        let mut session = Session::new(metadata());
        let now = time("2026-10-03T12:00:00+02:00");
        session.record(EventKind::GameStart, now);
        session.start_note(now);
        session.record(EventKind::GoalAway, now);
        session.entries[1].text = "unfinished observation".into();
        session.delete(0);
        assert_eq!(session.active_note, Some(1));
        session.delete(2);
        session.delete(99);
        assert_eq!(session.active_note, Some(1));
        session.delete(1);
        assert_eq!(session.active_note, None);
        assert_eq!(session.entries.len(), 3);
        assert!(session.entries.iter().all(|entry| entry.undone));
        assert!(!session.markdown().contains("unfinished observation"));
        let encoded = serde_json::to_string(&session).unwrap();
        let mut restored: Session = serde_json::from_str(&encoded).unwrap();
        restored.restore(1);
        assert_eq!(restored.entries[1].text, "unfinished observation");
        assert_eq!(restored.entries[1].timestamp, now);
        assert_eq!(restored.active_note, None);
        assert!(restored.markdown().contains("unfinished observation"));
    }

    #[test]
    fn score_sync_and_goal_removal_restore_independently() {
        let mut session = Session::new(metadata());
        let now = time("2026-10-03T12:00:00+02:00");
        session.record(EventKind::Custom, now);
        session.entries[0].score_delta = Some([3, 2]);
        session.record(EventKind::GoalAway, now);
        assert_eq!(session.score(), (3, 3));
        session.delete(1);
        assert_eq!(session.score(), (3, 2));
        session.delete(0);
        assert_eq!(session.score(), (0, 0));
        session.restore(1);
        assert_eq!(session.score(), (0, 1));
        session.restore(0);
        assert_eq!(session.score(), (3, 3));
        session.record(EventKind::Custom, now);
        session.entries[2].score_delta = Some([-3, -3]);
        assert_eq!(session.score(), (0, 0));
        session.delete(1);
        assert_eq!(session.score(), (0, 0));
        session.restore(1);
        session.delete(2);
        assert_eq!(session.score(), (3, 3));
        assert!(session.markdown().contains("Goal HTWK — HULKs 3:3 HTWK"));
        let saved = serde_json::to_string(&session).unwrap();
        assert_eq!(serde_json::from_str::<Session>(&saved).unwrap(), session);
    }

    #[test]
    fn removing_and_restoring_test_goals_never_changes_the_match_score() {
        let mut session = Session::new(metadata());
        let now = time("2026-10-03T12:00:00+02:00");
        for kind in [
            EventKind::GoldenGoalStart,
            EventKind::GoalHome,
            EventKind::GameStart,
            EventKind::GoalAway,
        ] {
            session.record(kind, now);
        }
        session.delete(1);
        assert_eq!(session.golden_goal_score(), (0, 0));
        assert_eq!(session.score(), (0, 1));
        session.restore(1);
        assert_eq!(session.golden_goal_score(), (1, 0));
        assert_eq!(session.score(), (0, 1));
    }

    #[test]
    fn combined_scopes_keep_scores_and_endings_independent_through_corrections() {
        let now = time("2026-10-03T12:00:00+02:00");
        let mut session = Session::new_match(metadata(), MatchType::Combined);
        assert_eq!(session.current_game(), MatchType::GoldenGoal);
        assert!(session.in_golden_goal());
        session.record(EventKind::GoldenGoalStart, now);
        session.record(EventKind::GoalHome, now);
        session.record(EventKind::Custom, now);
        session.entries[2].score_delta = Some([2, -1]);
        session.entries[2].from_controller = true;
        session.record(EventKind::GoalAway, now);
        session.entries[3].from_controller = true;
        session.record(EventKind::GameEnd, now);
        assert!(session.is_finished());
        assert!(session.is_game_finished(MatchType::GoldenGoal));
        assert!(!session.is_game_finished(MatchType::Official));
        assert_eq!(session.golden_goal_score(), (3, 0));
        assert_eq!(session.score(), (0, 0));

        session.record(EventKind::GameStart, now);
        session.entries[5].game = Some(MatchType::Official);
        session.record(EventKind::GoalAway, now);
        session.record(EventKind::Custom, now);
        session.entries[7].score_delta = Some([1, 2]);
        assert_eq!(session.current_game(), MatchType::Official);
        assert!(!session.in_golden_goal());
        assert!(!session.is_finished());
        assert_eq!(session.score(), (1, 3));
        assert_eq!(session.raw_score(), (1, 3));
        assert_eq!(session.raw_score_for(MatchType::GoldenGoal), (3, 0));
        session.record(EventKind::GameEnd, now);
        assert!(session.is_finished());
        assert!(session.is_game_finished(MatchType::GoldenGoal));
        assert!(session.is_game_finished(MatchType::Official));
        assert!(session.validate().is_ok());

        session.entries[1].kind = EventKind::GoalAway;
        assert_eq!(session.entries[1].game, Some(MatchType::GoldenGoal));
        assert_eq!(session.golden_goal_score(), (2, 1));
        session.delete(0);
        session.delete(5);
        assert_eq!(session.golden_goal_score(), (2, 1));
        assert_eq!(session.score(), (1, 3));
        session.delete(2);
        assert_eq!(session.golden_goal_score(), (0, 2));
        assert_eq!(session.score(), (1, 3));
        session.restore(2);
        session.delete(7);
        assert_eq!(session.score(), (0, 1));
        assert_eq!(session.golden_goal_score(), (2, 1));
        session.restore(7);
        session.delete(8);
        assert!(!session.is_finished());
        assert!(session.is_game_finished(MatchType::GoldenGoal));
        session.restore(8);
        assert!(session.is_finished());

        for index in 5..session.entries.len() {
            session.delete(index);
        }
        assert_eq!(session.current_game(), MatchType::GoldenGoal);
        assert!(session.is_finished());
        assert_eq!(session.score(), (0, 0));
        assert_eq!(session.golden_goal_score(), (2, 1));
        session.delete(4);
        assert!(!session.is_finished());
        session.restore(4);
        session.restore(6);
        assert_eq!(session.current_game(), MatchType::Official);
        assert!(!session.is_finished());
        assert_eq!(session.score(), (0, 1));
        assert_eq!(session.golden_goal_score(), (2, 1));
        assert!(!session.is_game_finished(MatchType::Combined));
    }

    #[test]
    fn combined_markdown_shares_metadata_but_separates_scores_budgets_and_clocks() {
        let now = time("2026-10-03T12:00:00.123456789+02:00");
        for hulks_home in [true, false] {
            let mut metadata = metadata();
            if !hulks_home {
                std::mem::swap(&mut metadata.home, &mut metadata.away);
            }
            let mut session = Session::new_match(metadata, MatchType::Combined);
            session.record(EventKind::GoldenGoalStart, now);
            session.entries[0].gc_seconds_remaining = Some(600);
            session.record(EventKind::GoalHome, now);
            session.entries[1].gc_seconds_remaining = Some(599);
            session.record(EventKind::Custom, now);
            session.entries[2].score_delta = Some([2, 1]);
            session.entries[2].from_controller = true;
            session.entries[2].text =
                GC_SNAPSHOT.replacen("GC received:", "GC sync — received:", 1);
            session.record(EventKind::GoalHome, now);
            session.entries[3].from_controller = true;
            session.record(EventKind::GameEnd, now);
            session.entries[4].gc_seconds_remaining = Some(0);
            session.entries[4].message_budget = Some(222);
            session.delete(4);
            session.record(EventKind::GoalAway, now);
            session.entries[5].gc_seconds_remaining = Some(-1);
            session.restore(4);
            assert!(!session.markdown().contains("Halbzeit"));

            session.record(EventKind::GameStart, now);
            session.entries[6].game = Some(MatchType::Official);
            session.entries[6].gc_seconds_remaining = Some(600);
            session.record(EventKind::GoalHome, now);
            session.record(EventKind::HalfTime, now);
            session.entries[8].message_budget = Some(111);
            session.record(EventKind::SecondHalfStart, now);
            session.record(EventKind::GoalAway, now);
            session.entries[10].gc_seconds_remaining = Some(-61);
            session.record(EventKind::GameEnd, now);
            session.entries[11].message_budget = Some(99);
            assert!(session.validate().is_ok());

            let markdown = session.markdown();
            let (test_home, test_away) = session.metadata.golden_goal_teams();
            let home = &session.metadata.home;
            let away = &session.metadata.away;
            assert!(
                markdown.starts_with(&format!("# Log 03.10.2026 hobbymesse {home} vs {away}\n"))
            );
            assert!(markdown.contains(&format!(
                "Golden Goal final score: {test_home} 4:2 {test_away}"
            )));
            assert!(markdown.contains(&format!("Half-time score: {home} 1:0 {away}")));
            assert!(markdown.contains(&format!("Final score: {home} 1:1 {away}")));
            assert!(markdown.contains(&format!("Goal {away} — {home} 1:1 {away} [-01:01]")));
            assert!(markdown.contains("[10:00] Golden Goal test"));
            assert!(markdown.contains("[10:00] Game start"));
            assert!(markdown.contains("[GC time unavailable] Score sync"));
            assert!(markdown.contains("HULKs remaining message budget: 222"));
            assert!(markdown.contains("HULKs remaining message budget: 111"));
            assert!(markdown.contains("HULKs remaining message budget: 99"));
            assert!(!markdown.contains("2026-10-03T12:00:00"));
            assert_eq!(markdown.matches("## Robot Assignments").count(), 1);
            assert_eq!(markdown.matches("## Golden Goal").count(), 1);
            assert_eq!(markdown.matches("## 1. Halbzeit").count(), 1);
            assert_eq!(markdown.matches("## 2. Halbzeit").count(), 1);
            assert_eq!(markdown.matches("Golden Goal final score:").count(), 1);
            assert_eq!(markdown.matches(MARKDOWN_MARKER).count(), 1);
            assert!(
                markdown.find("Golden Goal final score:").unwrap()
                    < markdown.find("## 1. Halbzeit").unwrap()
            );
            assert_eq!(session.golden_goal_score(), (4, 2));
            assert_eq!(session.score(), (1, 1));
        }
    }

    #[test]
    fn combined_json_preserves_scopes_opponent_and_signed_scores() {
        let now = time("2026-10-03T12:00:00.123456789+02:00");
        let mut session = Session::new_match(metadata(), MatchType::Combined);
        session.golden_goal_opponent = Some(42);
        session.record(EventKind::Custom, now);
        session.entries[0].score_delta = Some([-2, 3]);
        session.entries[0].gc_seconds_remaining = Some(-61);
        session.record(EventKind::GameEnd, now);
        session.record(EventKind::GameStart, now);
        session.entries[2].game = Some(MatchType::Official);
        session.record(EventKind::Custom, now);
        session.entries[3].score_delta = Some([4, -5]);
        let json = serde_json::to_value(&session).unwrap();
        assert_eq!(json["match_type"], "Combined");
        assert_eq!(json["golden_goal_opponent"], 42);
        assert_eq!(json["metadata"]["away"], "HTWK");
        assert_eq!(json["entries"][0]["game"], "GoldenGoal");
        assert_eq!(json["entries"][2]["game"], "Official");
        assert_eq!(
            json["entries"][0]["timestamp"],
            "2026-10-03T12:00:00.123456789+02:00"
        );
        assert_eq!(json["entries"][0]["gc_seconds_remaining"], -61);
        let restored: Session = serde_json::from_value(json).unwrap();
        assert_eq!(restored, session);
        assert!(restored.validate().is_ok());
        assert_eq!(restored.markdown(), session.markdown());
        assert_eq!(restored.current_game(), MatchType::Official);
        assert_eq!(restored.raw_score_for(MatchType::GoldenGoal), (-2, 3));
        assert_eq!(restored.golden_goal_score(), (0, 3));
        assert_eq!(restored.raw_score(), (4, -5));
        assert_eq!(restored.score(), (4, 0));

        for match_type in [None, Some(MatchType::Official), Some(MatchType::GoldenGoal)] {
            let mut legacy = Session::new(metadata());
            legacy.match_type = match_type;
            legacy.record(EventKind::GoalHome, now);
            let json = serde_json::to_value(&legacy).unwrap();
            assert!(json.get("golden_goal_opponent").is_none());
            assert!(json["entries"][0].get("game").is_none());
            let restored: Session = serde_json::from_value(json).unwrap();
            assert_eq!(restored, legacy);
            assert_eq!(restored.golden_goal_opponent, None);
            assert_eq!(restored.entries[0].game, None);
            assert_eq!(restored.raw_score_for(restored.current_game()), (1, 0));
        }
    }

    #[test]
    fn combined_official_pre_start_notes_are_not_under_the_golden_goal_heading() {
        let mut session = Session::new_match(metadata(), MatchType::Combined);
        let now = time("2026-10-04T12:00:00+02:00");
        session.record(EventKind::GameEnd, now);
        session.record(EventKind::Note, now);
        let entry = session.entries.last_mut().unwrap();
        entry.game = Some(MatchType::Official);
        entry.text = "official preparation".into();
        let markdown = session.markdown();
        assert!(
            markdown.find("Golden Goal final score").unwrap()
                < markdown.find("## Official match").unwrap()
        );
        assert!(
            markdown.find("## Official match").unwrap()
                < markdown.find("official preparation").unwrap()
        );
        assert!(!markdown.contains("Game start"));
    }

    #[test]
    fn combined_validation_requires_a_real_game_scope_and_matching_phases() {
        let now = time("2026-10-03T12:00:00+02:00");
        let mut session = Session::new_match(metadata(), MatchType::Combined);
        session.record(EventKind::GoldenGoalStart, now);
        assert!(session.validate().is_ok());
        for game in [None, Some(MatchType::Combined), Some(MatchType::Official)] {
            session.entries[0].game = game;
            assert!(session.validate().is_err());
        }
        session.entries[0].game = Some(MatchType::GoldenGoal);
        for kind in [
            EventKind::GameStart,
            EventKind::HalfTime,
            EventKind::SecondHalfStart,
        ] {
            session.entries[0].kind = kind;
            assert!(session.validate().is_err());
            session.entries[0].game = Some(MatchType::Official);
            assert!(session.validate().is_ok());
            session.entries[0].game = Some(MatchType::GoldenGoal);
        }
    }

    #[test]
    fn fixed_matches_keep_manual_and_controller_scores_in_their_own_game() {
        let now = time("2026-10-03T12:00:00+02:00");
        let mut official = Session::new_match(metadata(), MatchType::Official);
        official.record(EventKind::GoalHome, now);
        for hulks_home in [true, false] {
            let mut metadata = metadata();
            if hulks_home {
                metadata.away = "Invisibles".into();
            } else {
                metadata.home = "Invisibles".into();
                metadata.away = "HULKs".into();
            }
            let mut golden = Session::new_match(metadata, MatchType::GoldenGoal);
            assert!(golden.in_golden_goal());
            assert!(!golden.is_finished());
            assert_eq!(golden.score(), (0, 0));
            assert_eq!(
                golden.current_teams(),
                if hulks_home {
                    ("HULKs", "Invisibles")
                } else {
                    ("Invisibles", "HULKs")
                }
            );
            assert!(golden.markdown().contains("## Golden Goal\n"));
            golden.record(EventKind::GoalHome, now);
            golden.record(EventKind::GoldenGoalStart, now);
            golden.record(EventKind::Custom, now);
            golden.entries[2].from_controller = true;
            golden.entries[2].score_delta = Some([3, 2]);
            golden.entries[2].text = GC_SNAPSHOT.replacen("GC received:", "GC sync — received:", 1);
            golden.record(EventKind::GoalAway, now);
            golden.entries[3].from_controller = true;
            golden.record(EventKind::Custom, now);
            golden.entries[4].from_controller = true;
            golden.entries[4].score_delta = Some([-1, 0]);
            golden.record(EventKind::GoalAway, now);
            golden.record(EventKind::GoalHome, now);
            golden.entries[6].from_controller = true;
            golden.record(EventKind::GameEnd, now);
            golden.entries[7].from_controller = true;
            golden.entries[7].message_budget = Some(321);
            golden.entries[7].golden_goal_message_budget = Some(999);

            assert!(golden.validate().is_ok());
            assert!(golden.in_golden_goal());
            assert!(golden.is_finished());
            assert_eq!(golden.raw_score(), (4, 4));
            assert_eq!(golden.score(), (4, 4));
            assert_eq!(golden.golden_goal_score(), (4, 4));
            assert_eq!(official.score(), (1, 0));
            assert_eq!(official.golden_goal_score(), (0, 0));
            assert!(!official.in_golden_goal());
            let markdown = golden.markdown();
            let (home, away) = golden.current_teams();
            assert!(markdown.contains(&format!("Golden Goal final score: {home} 4:4 {away}")));
            assert!(markdown.contains("HULKs remaining message budget: 321"));
            assert!(!markdown.contains("999"));
            assert!(markdown.contains("Score sync"));
            assert!(markdown.contains("Score correction"));
            assert!(!markdown.contains("GC received:"));
            assert!(!markdown.contains("Halbzeit"));
            assert_eq!(markdown.matches("## Golden Goal").count(), 1);
            assert_eq!(markdown.matches("Golden Goal final score:").count(), 1);
            assert!(!markdown.contains("Golden Goal score:"));

            golden.delete(1);
            assert!(golden.in_golden_goal());
            assert_eq!(golden.score(), (4, 4));
            golden.delete(2);
            assert_eq!(golden.score(), (1, 2));
            golden.delete(6);
            assert_eq!(golden.score(), (0, 2));
            golden.restore(2);
            golden.restore(6);
            assert_eq!(golden.score(), (4, 4));
            assert_eq!(official.score(), (1, 0));
        }
    }

    #[test]
    fn fixed_match_validation_rejects_other_match_phases_and_wrong_golden_opponents() {
        let now = time("2026-10-03T12:00:00+02:00");
        let mut official = Session::new_match(metadata(), MatchType::Official);
        for kind in [
            EventKind::GameStart,
            EventKind::HalfTime,
            EventKind::SecondHalfStart,
            EventKind::GameEnd,
        ] {
            official.record(kind, now);
        }
        assert!(official.validate().is_ok());
        official.record(EventKind::GoldenGoalStart, now);
        assert!(official.validate().is_err());

        let mut golden = Session::new_match(metadata(), MatchType::GoldenGoal);
        assert_eq!(
            golden.validate().unwrap_err(),
            "Golden Goal opponent must be Invisibles"
        );
        golden.metadata.away = " iNvIsIbLeS ".into();
        assert!(golden.validate().is_ok());
        golden.record(EventKind::GoldenGoalStart, now);
        golden.record(EventKind::GameEnd, now);
        assert!(golden.validate().is_ok());
        for kind in [
            EventKind::GameStart,
            EventKind::HalfTime,
            EventKind::SecondHalfStart,
        ] {
            golden.entries[0].kind = kind;
            assert!(golden.validate().is_err());
        }
        golden.entries[0].kind = EventKind::GoldenGoalStart;
        golden.metadata.home = "invisibles".into();
        golden.metadata.away = "HULKs".into();
        assert!(golden.validate().is_ok());
        golden.metadata.home = "HTWK".into();
        assert!(golden.validate().is_err());
    }

    #[test]
    fn removing_and_restoring_game_ends_reopens_and_closes_sessions() {
        let now = time("2026-10-03T12:00:00+02:00");
        for match_type in [None, Some(MatchType::Official), Some(MatchType::GoldenGoal)] {
            let mut session = Session::new(metadata());
            session.match_type = match_type;
            if match_type == Some(MatchType::GoldenGoal) {
                session.metadata.away = "Invisibles".into();
            }
            session.record(EventKind::GoalHome, now);
            assert!(!session.is_finished());
            session.record(EventKind::GameEnd, now);
            session.entries[1].message_budget = Some(42);
            assert!(session.is_finished());
            let finished = session.markdown();
            session.delete(1);
            assert!(!session.is_finished());
            if match_type == Some(MatchType::GoldenGoal) {
                assert!(session.in_golden_goal());
                assert!(
                    session
                        .markdown()
                        .contains("Golden Goal score: HULKs 1:0 Invisibles")
                );
                assert!(!session.markdown().contains("Golden Goal final score:"));
                assert!(
                    !session
                        .markdown()
                        .contains("HULKs remaining message budget:")
                );
            }
            session.restore(1);
            assert!(session.is_finished());
            assert_eq!(session.markdown(), finished);
            session.record(EventKind::GameEnd, now);
            session.delete(1);
            assert!(session.is_finished());
            session.delete(2);
            assert!(!session.is_finished());
        }
    }

    #[test]
    fn fixed_match_gc_clocks_export_signed_countdowns_and_preserve_precise_json_time() {
        let now = time("2026-10-03T23:59:59.123456789+02:00");
        for match_type in [MatchType::Official, MatchType::GoldenGoal] {
            let mut metadata = metadata();
            if match_type == MatchType::GoldenGoal {
                metadata.away = "Invisibles".into();
            }
            let mut session = Session::new_match(metadata, match_type);
            for (seconds, expected) in [
                (Some(600), "10:00"),
                (Some(1), "00:01"),
                (Some(0), "00:00"),
                (Some(-1), "-00:01"),
                (Some(-61), "-01:01"),
                (Some(i16::MAX), "546:07"),
                (Some(i16::MIN), "-546:08"),
                (None, "GC time unavailable"),
            ] {
                session.record(EventKind::Note, now);
                let entry = session.entries.last_mut().unwrap();
                entry.gc_seconds_remaining = seconds;
                entry.text = format!("clock {expected}");
            }
            let markdown = session.markdown();
            for entry in &session.entries {
                let expected = entry.text.strip_prefix("clock ").unwrap();
                assert!(markdown.contains(&format!("- [{expected}] clock {expected}")));
            }
            assert!(!markdown.contains("2026-10-03T23:59:59"));
            let json = serde_json::to_value(&session).unwrap();
            assert_eq!(
                json["match_type"],
                serde_json::to_value(match_type).unwrap()
            );
            assert_eq!(
                json["entries"][0]["timestamp"],
                "2026-10-03T23:59:59.123456789+02:00"
            );
            assert_eq!(json["entries"][0]["gc_seconds_remaining"], 600);
            assert_eq!(json["entries"][6]["gc_seconds_remaining"], i16::MIN);
            assert!(
                json["entries"][7]
                    .get("gc_seconds_remaining")
                    .unwrap()
                    .is_null()
            );
            let restored: Session = serde_json::from_value(json).unwrap();
            assert_eq!(restored, session);
            assert_eq!(restored.markdown(), markdown);
        }
    }

    #[test]
    fn fixed_match_markdown_keeps_minimal_controller_filtering_and_sync_labels() {
        let mut session = Session::new_match(metadata(), MatchType::Official);
        let now = time("2026-10-03T12:00:00+02:00");
        session.record(EventKind::Custom, now);
        session.entries[0].from_controller = true;
        session.entries[0].text = GC_SNAPSHOT.into();
        session.entries[0].gc_seconds_remaining = Some(599);
        session.record(EventKind::GameStart, now);
        session.entries[1].from_controller = true;
        session.entries[1].text = GC_SNAPSHOT.replacen("GC received:", "GC sync — received:", 1);
        session.entries[1].gc_seconds_remaining = Some(598);
        session.record(EventKind::GoalHome, now);
        session.entries[2].from_controller = true;
        session.entries[2].text = GC_SNAPSHOT.into();
        session.entries[2].gc_seconds_remaining = Some(597);
        session.record(EventKind::Custom, now);
        session.entries[3].from_controller = true;
        session.entries[3].text = format!("{GC_SNAPSHOT}; observer annotation");
        session.entries[3].gc_seconds_remaining = Some(596);
        let markdown = session.markdown();
        assert!(!markdown.contains("[09:59]"));
        assert!(markdown.contains("[09:58] Game start — GC sync (original event time unknown)"));
        assert!(markdown.contains("Goal HULKs — HULKs 1:0 HTWK [09:57]"));
        assert!(markdown.contains("[09:56] Custom — GC received:"));
        assert!(markdown.contains("observer annotation"));
        assert_eq!(
            serde_json::to_value(&session).unwrap()["entries"]
                .as_array()
                .unwrap()
                .len(),
            4
        );
    }

    #[test]
    fn legacy_sessions_keep_wall_clock_markdown_even_with_captured_gc_time() {
        let mut session = Session::new(metadata());
        let now = time("2026-10-03T12:00:00.123+02:00");
        session.record(EventKind::GoldenGoalStart, now);
        session.record(EventKind::GoalHome, now);
        session.record(EventKind::GameStart, now);
        session.record(EventKind::GoalAway, now);
        session.record(EventKind::GameEnd, now);
        let markdown = session.markdown();
        for entry in &mut session.entries {
            entry.gc_seconds_remaining = Some(600);
        }
        assert_eq!(session.markdown(), markdown);
        assert!(markdown.contains("[2026-10-03T12:00:00.123+02:00]"));
        assert_eq!(session.score(), (0, 1));
        assert_eq!(session.golden_goal_score(), (1, 0));
        assert!(
            serde_json::to_value(&session)
                .unwrap()
                .get("match_type")
                .is_none()
        );
    }

    #[test]
    fn validation_rejects_invalid_drafts_and_terminal_controls() {
        let mut session = Session::new(metadata());
        assert!(session.validate().is_ok());
        session.metadata.home = " \n".into();
        assert!(session.validate().is_err());
        session.metadata = metadata();
        session.active_note = Some(0);
        assert!(session.validate().is_err());
        session.record(EventKind::GoalHome, time("2026-10-03T12:00:00+02:00"));
        assert!(session.validate().is_err());
        session.active_note = None;
        session.entries[0].text = "\u{1b}[2J".into();
        assert!(session.validate().is_err());
        session.entries[0].text = "normal\nmultiline".into();
        assert!(session.validate().is_ok());
        session.metadata.robots[0] = "42\n43".into();
        assert!(session.validate().is_err());
    }
}
