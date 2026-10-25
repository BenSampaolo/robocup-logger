use std::borrow::Cow;

use chrono::{DateTime, FixedOffset, NaiveDate, SecondsFormat};
use serde::{Deserialize, Serialize};

pub(crate) const MARKDOWN_MARKER: &str = "<!-- robocup-logger generated log -->\n";

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Metadata {
    pub event: String,
    pub home: String,
    pub away: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracked_team: Option<u8>,
    pub date: NaiveDate,
    pub label: String,
    pub robots: [String; 5],
}

impl Metadata {
    pub fn tracked_team_number(&self) -> u8 {
        self.tracked_team
            .unwrap_or(crate::game_controller::HULKS_TEAM_NUMBER)
    }

    pub fn tracked_are_home(&self) -> bool {
        crate::teams::team_number(&self.home) == Some(self.tracked_team_number())
    }

    pub fn tracked_team_name(&self) -> &str {
        if self.tracked_are_home() {
            &self.home
        } else {
            &self.away
        }
    }

    pub fn opponent_name(&self) -> &str {
        if self.tracked_are_home() {
            &self.away
        } else {
            &self.home
        }
    }

    pub fn golden_goal_teams(&self) -> (&str, &str) {
        if self.tracked_are_home() {
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
        if let Some(tracked) = self.tracked_team {
            let home =
                crate::teams::team_number(&self.home).ok_or("Choose a supported home team")?;
            let away =
                crate::teams::team_number(&self.away).ok_or("Choose a supported away team")?;
            if home == away {
                return Err("Tracked team and opponent must be different".into());
            }
            if tracked != home && tracked != away {
                return Err("Tracked team must be one of the match teams".into());
            }
        } else if self.home.trim().eq_ignore_ascii_case("HULKs")
            == self.away.trim().eq_ignore_ascii_case("HULKs")
        {
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

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PostgameNote {
    pub actionable: bool,
    pub title: String,
    pub robots: String,
    pub owner: String,
    pub discussion: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub postgame: Option<PostgameNote>,
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
    #[serde(default, skip_serializing_if = "is_false")]
    pub controller_description_edited: bool,
    #[serde(default)]
    pub undone: bool,
    #[serde(default)]
    pub from_controller: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_budget: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub golden_goal_message_budget: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score_delta: Option<[i32; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub statistics: Option<crate::statistics::Statistics>,
}

impl Entry {
    fn controller_snapshot_text(&self) -> Option<&str> {
        if !self.from_controller
            || self.controller_description_edited
            || self.kind == EventKind::Note
        {
            return None;
        }
        let text = self
            .text
            .strip_prefix("GC received: ")
            .or_else(|| self.text.strip_prefix("GC sync — received: "))?;
        let (_, clock) = text.rsplit_once("; secondary ")?;
        let clock = clock
            .strip_suffix("; phase observed on attachment, not the original transition time")
            .unwrap_or(clock);
        let (minutes, seconds) = clock.split_once(':')?;
        (minutes.parse::<i16>().is_ok()
            && seconds.len() == 2
            && seconds.parse::<u8>().is_ok_and(|seconds| seconds < 60))
        .then_some(text)
    }

    pub(crate) fn has_controller_snapshot(&self) -> bool {
        self.controller_snapshot_text().is_some()
    }

    pub(crate) fn is_controller_sync(&self) -> bool {
        self.from_controller && self.text.starts_with("GC sync — ")
    }

    pub(crate) fn controller_phase(&self) -> Option<&'static str> {
        let summary = self
            .controller_snapshot_text()?
            .split_once("; set play: ")?
            .0
            .rsplit_once("; ")?
            .1;
        let (phase, rest) = summary.split_once(" / ")?;
        let (state, half) = rest.split_once(" / ")?;
        if !matches!(state, "Initial" | "Ready" | "Set" | "Playing" | "Finished")
            || !matches!(half, "first half" | "second half")
        {
            return None;
        }
        match (phase, state, half) {
            ("Normal", "Finished", "first half") => Some("half-time"),
            ("Normal", "Finished", "second half") => Some("full-time"),
            ("Normal", _, "first half") => Some("first half"),
            ("Normal", _, "second half") => Some("second half"),
            ("Extra time", _, "first half") => Some("extra time · first half"),
            ("Extra time", _, "second half") => Some("extra time · second half"),
            ("Penalty shootout", _, _) => Some("penalty shootout"),
            ("Timeout", _, _) => Some("timeout"),
            _ => None,
        }
    }

    pub(crate) fn display_text(&self) -> String {
        let Some(text) = self.controller_snapshot_text() else {
            return self.text.clone();
        };
        let score = text.split(';').next().unwrap_or_default();
        let penalties = self
            .statistics
            .as_ref()
            .map(|stats| stats.penalties.as_slice())
            .unwrap_or_default();
        if !penalties.is_empty() {
            format!(
                "Penalties · {}",
                penalties
                    .iter()
                    .map(|change| format!(
                        "player {}: {}→{}",
                        change.player, change.before, change.after
                    ))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        } else if self.kind == EventKind::Custom {
            format!("Score sync · {score}")
        } else {
            format!("{} · {score}", self.kind.label())
        }
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field_dimensions: Option<crate::field::FieldDimensions>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replay_position: Option<f64>,
    pub metadata: Metadata,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub match_type: Option<MatchType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub golden_goal_opponent: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub official_opponent: Option<u8>,
    pub entries: Vec<Entry>,
    pub active_note: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub postgame_position: Option<usize>,
}

impl Session {
    pub fn new(metadata: Metadata) -> Self {
        Self {
            capture_session: None,
            field_dimensions: None,
            replay_position: None,
            metadata,
            match_type: None,
            golden_goal_opponent: None,
            official_opponent: None,
            entries: Vec::new(),
            active_note: None,
            postgame_position: None,
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
            postgame: None,
            capture_id: None,
            capture_after: None,
            game: (self.match_type == Some(MatchType::Combined)).then(|| self.current_game()),
            timestamp: now,
            gc_seconds_remaining: None,
            kind,
            text: String::new(),
            controller_description_edited: false,
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

    pub(crate) fn teams_for(&self, game: MatchType) -> (Cow<'_, str>, Cow<'_, str>) {
        if game == MatchType::GoldenGoal {
            let (home, away) = self.metadata.golden_goal_teams();
            return (Cow::Borrowed(home), Cow::Borrowed(away));
        }
        let tracked = Cow::Borrowed(self.metadata.tracked_team_name());
        let opponent = match self.official_opponent {
            Some(opponent) => Cow::Owned(crate::teams::display_name(opponent)),
            None => Cow::Borrowed(self.metadata.opponent_name()),
        };
        if self.metadata.tracked_are_home() {
            (tracked, opponent)
        } else {
            (opponent, tracked)
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
        if let Some(dimensions) = &self.field_dimensions {
            dimensions.validate()?;
        }
        if self
            .replay_position
            .is_some_and(|position| !position.is_finite() || position < 0.0)
        {
            return Err("Replay position must be finite and nonnegative".into());
        }
        if matches!(
            self.match_type,
            Some(MatchType::GoldenGoal | MatchType::Combined)
        ) && self.metadata.tracked_team_number() == 0
        {
            return Err("Invisibles cannot be the tracked team in Golden Goal".into());
        }
        if self.match_type == Some(MatchType::GoldenGoal) {
            let opponent = self.metadata.opponent_name();
            if !opponent.trim().eq_ignore_ascii_case("Invisibles") {
                return Err("Golden Goal opponent must be Invisibles".into());
            }
        }
        if self
            .postgame_position
            .is_some_and(|index| index >= self.entries.len())
        {
            return Err("Postgame position must refer to an existing entry".into());
        }
        if let Some(index) = self.active_note {
            match self.entries.get(index) {
                Some(entry) if entry.kind == EventKind::Note && !entry.undone => {}
                _ => return Err("Active note must refer to an existing note".into()),
            }
        }
        for entry in &self.entries {
            validate_text("Entry text", &entry.text, true)?;
            if let Some(note) = &entry.postgame {
                for (name, value) in [
                    ("Issue title", &note.title),
                    ("Robots", &note.robots),
                    ("Owner", &note.owner),
                    ("Discussion", &note.discussion),
                ] {
                    validate_text(name, value, true)?;
                }
            }
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
        let (home, away) = self.teams_for(if fixed_golden_goal {
            MatchType::GoldenGoal
        } else {
            MatchType::Official
        });
        let home = markdown_text(home.as_ref());
        let away = markdown_text(away.as_ref());
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
        let (home, away) = self.teams_for(if fixed_golden_goal {
            MatchType::GoldenGoal
        } else {
            MatchType::Official
        });
        let home = markdown_text(home.as_ref());
        let away = markdown_text(away.as_ref());
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
            let gc_sync = gc_snapshot && entry.is_controller_sync();
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
                    output.push_str(&message_budget_summary(
                        entry.golden_goal_message_budget,
                        self.metadata.tracked_team_name(),
                    ));
                    in_golden_goal = false;
                }
                _ => {}
            }
            let heading = match entry.kind {
                _ if fixed_golden_goal => None,
                EventKind::GoldenGoalStart => Some("Golden Goal"),
                EventKind::GameStart => Some("First half"),
                EventKind::HalfTime => Some("Half-time"),
                EventKind::SecondHalfStart => Some("Second half"),
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
                output.push_str(&message_budget_summary(
                    entry.message_budget,
                    self.metadata.tracked_team_name(),
                ));
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

fn message_budget_summary(budget: Option<u16>, team: &str) -> String {
    format!(
        "{} remaining message budget: {}\n\n",
        markdown_text(team),
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
