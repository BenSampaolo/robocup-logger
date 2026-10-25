use std::time::Instant;

use ratatui::{Frame, layout::Rect, style::Color};

use crate::{
    arcade::{self, Cue, Effect},
    game_controller::Snapshot,
    model::{Entry, EventKind, MatchType, Session},
};

#[derive(Default)]
pub(crate) struct Arcade {
    seen: usize,
    current: Option<Cue>,
}

impl Arcade {
    pub fn new(seen: usize) -> Self {
        Self {
            seen,
            current: None,
        }
    }

    pub fn observe(&mut self, session: &Session, now: Instant) -> bool {
        let mut changed = false;
        if self
            .current
            .as_ref()
            .is_some_and(|cue| now.duration_since(cue.started) >= cue.duration())
        {
            self.current = None;
        }
        for entry in &session.entries[self.seen.min(session.entries.len())..] {
            if let Some(cue) = Cue::from_entry(session, entry, now) {
                self.current = Some(cue);
                changed = true;
            }
        }
        self.seen = session.entries.len();
        changed
    }

    pub fn observe_controller(
        &mut self,
        session: &Session,
        previous: Option<&Snapshot>,
        snapshot: &Snapshot,
        now: Instant,
    ) {
        if self.observe(session, now) {
            return;
        }
        let Some(previous) = previous.filter(|old| old.opponent_number == snapshot.opponent_number)
        else {
            return;
        };
        if let Some(cue) = Cue::from_controller(previous, snapshot, now) {
            self.current = Some(cue);
        }
    }

    pub fn draw(&self, frame: &mut Frame, area: Rect, now: Instant, gc_clock: Option<String>) {
        arcade::draw(frame, area, self.current.as_ref(), now, gc_clock);
    }
}

impl Cue {
    pub(crate) fn from_controller(
        previous: &Snapshot,
        snapshot: &Snapshot,
        started: Instant,
    ) -> Option<Self> {
        let phase = (previous.game_phase != snapshot.game_phase)
            .then(|| match snapshot.game_phase {
                1 => Some((
                    "SHOOTOUT",
                    Effect::Spotlight,
                    Color::LightMagenta,
                    "PENALTY SHOOTOUT",
                )),
                2 => Some((
                    "EXTRA TIME",
                    Effect::Aurora,
                    Color::LightCyan,
                    "EXTRA PERIOD",
                )),
                3 => Some(("TIMEOUT", Effect::Orbit, Color::LightMagenta, "TIME OUT")),
                _ => None,
            })
            .flatten();
        let restart = ((previous.set_play, previous.kicking_team)
            != (snapshot.set_play, snapshot.kicking_team))
            .then(|| match snapshot.set_play {
                1 => Some(("DIRECT FREE", Effect::Laser, Color::LightYellow)),
                2 => Some(("INDIRECT FREE", Effect::Scramble, Color::LightMagenta)),
                3 => Some(("PENALTY KICK", Effect::Target, Color::LightRed)),
                4 => Some(("KICK IN", Effect::Fluid, Color::LightCyan)),
                5 => Some(("GOAL KICK", Effect::Launch, Color::LightGreen)),
                6 => Some(("CORNER KICK", Effect::Ricochet, Color::Rgb(255, 160, 65))),
                _ => None,
            })
            .flatten();
        let cleared: Vec<_> = previous
            .tracked_penalties
            .iter()
            .zip(snapshot.tracked_penalties)
            .enumerate()
            .filter(|(_, (before, after))| **before != 0 && *after == 0)
            .map(|(player, _)| {
                format!("{} {}", snapshot.jersey_color(player as u8 + 1), player + 1)
            })
            .collect();
        let (title, effect, color, detail) = if let Some((title, effect, color, detail)) = phase {
            (title, effect, color, detail.to_owned())
        } else if let Some((title, effect, color)) = restart {
            (
                title,
                effect,
                color,
                if snapshot.kicking_team == 255 {
                    "Awaiting team".into()
                } else {
                    crate::teams::display_name(snapshot.kicking_team)
                },
            )
        } else if snapshot.ball_became_free(previous) {
            (
                "BALL FREE",
                Effect::Ripple,
                Color::LightCyan,
                "PLAY ON".into(),
            )
        } else if !cleared.is_empty() {
            (
                "RETURN",
                Effect::Sparkle,
                Color::LightGreen,
                cleared.join(" · "),
            )
        } else if snapshot.stopped && !previous.stopped {
            (
                "STOPPED",
                Effect::Freeze,
                Color::LightRed,
                "GC CLOCK STOPPED".into(),
            )
        } else if snapshot.state == 1 && previous.state != 1 {
            (
                "READY",
                Effect::Scan,
                Color::LightBlue,
                "ROBOTS POSITIONING".into(),
            )
        } else if snapshot.state == 2 && previous.state != 2 {
            (
                "SET",
                Effect::Gate,
                Color::LightYellow,
                "AWAITING PLAY".into(),
            )
        } else if snapshot.state == 3
            && !snapshot.stopped
            && (previous.state != 3
                || previous.stopped
                || (previous.game_phase == 3 && snapshot.game_phase != 3))
        {
            ("RESUME", Effect::Noise, Color::LightGreen, "PLAYING".into())
        } else {
            return None;
        };
        Some(Self {
            effect,
            title: title.into(),
            detail,
            color,
            started,
        })
    }

    pub(crate) fn from_entry(session: &Session, entry: &Entry, started: Instant) -> Option<Self> {
        if entry.undone || (entry.from_controller && entry.text.starts_with("GC sync — ")) {
            return None;
        }
        let golden_goal =
            entry.game.unwrap_or_else(|| session.current_game()) == MatchType::GoldenGoal;
        let (home, away) = if golden_goal {
            session.metadata.golden_goal_teams()
        } else {
            let tracked = session.metadata.tracked_team_name();
            let opponent = session
                .official_opponent
                .and_then(crate::teams::team_name)
                .unwrap_or_else(|| session.metadata.opponent_name());
            if session.metadata.tracked_are_home() {
                (tracked, opponent)
            } else {
                (opponent, tracked)
            }
        };
        let home_color = if session.metadata.tracked_are_home() {
            Color::LightGreen
        } else {
            Color::LightRed
        };
        let away_color = if session.metadata.tracked_are_home() {
            Color::LightRed
        } else {
            Color::LightGreen
        };
        let (title, color, detail) = match entry.kind {
            EventKind::GoalHome => ("GOAL!", home_color, home.to_owned()),
            EventKind::GoalAway => ("GOAL!", away_color, away.to_owned()),
            EventKind::GameStart => ("KICKOFF", Color::LightCyan, "LET'S PLAY".into()),
            EventKind::GoldenGoalStart => {
                ("GOLDEN GOAL", Color::LightYellow, "DEPLOYMENT TEST".into())
            }
            EventKind::SecondHalfStart => {
                ("SECOND HALF", Color::LightCyan, "BACK IN THE GAME".into())
            }
            EventKind::HalfTime => ("HALF TIME", Color::LightBlue, "TAKE A BREATHER".into()),
            EventKind::GameEnd if golden_goal => {
                ("TEST COMPLETE", Color::LightGreen, "GOLDEN GOAL".into())
            }
            EventKind::GameEnd => ("FULL TIME", Color::LightMagenta, "MATCH COMPLETE".into()),
            _ => {
                let players: Vec<_> = entry
                    .statistics
                    .as_ref()?
                    .penalties
                    .iter()
                    .filter(|change| change.before == 0 && change.after != 0)
                    .map(|change| change.player.to_string())
                    .collect();
                if players.is_empty() {
                    return None;
                }
                (
                    "PENALTY",
                    Color::LightYellow,
                    format!(
                        "{} · #{}",
                        session.metadata.tracked_team_name(),
                        players.join(", #")
                    ),
                )
            }
        };
        let effect = match entry.kind {
            EventKind::GoalHome | EventKind::GoalAway => {
                if (entry.kind == EventKind::GoalHome) == session.metadata.tracked_are_home() {
                    Effect::Ticker
                } else {
                    Effect::Concede
                }
            }
            EventKind::GameStart => Effect::Speed,
            EventKind::GoldenGoalStart => Effect::Flame,
            EventKind::SecondHalfStart => Effect::Split,
            EventKind::HalfTime => Effect::Mirror,
            EventKind::GameEnd if golden_goal => Effect::Tetris,
            EventKind::GameEnd => Effect::Fireworks,
            _ => Effect::Whistle,
        };
        Some(Self {
            effect,
            title: title.into(),
            detail,
            color,
            started,
        })
    }
}
