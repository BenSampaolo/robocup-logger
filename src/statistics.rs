use serde::{Deserialize, Serialize};

use crate::model::{Entry, EventKind, MatchType, Session, markdown_text};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Statistics {
    pub baseline: Option<StatisticsBaseline>,
    pub penalties: Vec<PenaltyChange>,
    pub opponent_restart: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct StatisticsBaseline {
    pub penalties: [u8; 20],
    pub pre_game: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PenaltyChange {
    pub player: u8,
    pub before: u8,
    pub after: u8,
}

#[derive(Clone, Copy, Default)]
struct Penalties {
    total: usize,
    incapable: usize,
}

struct Aggregate {
    penalties: [Penalties; 20],
    codes: [u8; 20],
    counted_episode: [bool; 20],
    seen_penalty: [bool; 20],
    opponent_restarts: usize,
    peak_penalties: usize,
    longest_run: usize,
    current_run: usize,
    run_available: bool,
    complete: bool,
    observed: bool,
    ended: bool,
}

fn is_penalty(code: u8) -> Option<bool> {
    match code {
        0 | 11 | 13 => Some(false),
        1..=10 | 12 => Some(true),
        _ => None,
    }
}

impl Aggregate {
    fn from_session(session: &Session) -> Self {
        let mut aggregate = Self {
            penalties: [Penalties::default(); 20],
            codes: [0; 20],
            counted_episode: [false; 20],
            seen_penalty: [false; 20],
            opponent_restarts: 0,
            peak_penalties: 0,
            longest_run: 0,
            current_run: 0,
            run_available: true,
            complete: true,
            observed: false,
            ended: false,
        };
        let mut in_golden_goal = false;
        for entry in session.entries.iter().filter(|entry| !entry.undone) {
            let official = match session.match_type {
                Some(MatchType::Official) => true,
                Some(MatchType::GoldenGoal) => false,
                Some(MatchType::Combined) => entry.game == Some(MatchType::Official),
                None => {
                    if entry.kind == EventKind::GoldenGoalStart {
                        in_golden_goal = true;
                    } else if entry.ends_golden_goal() {
                        in_golden_goal = false;
                    }
                    entry.kind != EventKind::GoldenGoalStart
                        && (!in_golden_goal || entry.from_controller)
                }
            };
            if official {
                aggregate.entry(entry, session.metadata.tracked_are_home());
            }
        }
        aggregate
    }

    fn entry(&mut self, entry: &Entry, tracked_are_home: bool) {
        let goal = matches!(entry.kind, EventKind::GoalHome | EventKind::GoalAway);
        let score_changed = entry.score_delta.is_some_and(|delta| delta != [0, 0]);
        if (goal || score_changed) && !self.observed {
            self.complete = false;
        }
        if entry.from_controller && entry.statistics.is_none() {
            self.complete = false;
        }
        if score_changed || (goal && entry.gc_seconds_remaining.is_none()) {
            self.run_available = false;
        }
        if entry.kind == EventKind::GameEnd {
            self.ended = true;
            if entry.gc_seconds_remaining.is_none() {
                self.complete = false;
            }
        }
        if let Some(statistics) = &entry.statistics {
            self.observe(statistics);
        }
        if goal {
            if (entry.kind == EventKind::GoalHome) == tracked_are_home {
                self.current_run += 1;
                self.longest_run = self.longest_run.max(self.current_run);
            } else {
                self.current_run = 0;
            }
        }
    }

    fn observe(&mut self, statistics: &Statistics) {
        if let Some(baseline) = &statistics.baseline {
            if self.observed
                || !baseline.pre_game
                || baseline
                    .penalties
                    .iter()
                    .any(|&code| is_penalty(code) != Some(false))
            {
                self.complete = false;
            }
            self.codes = baseline.penalties;
            self.counted_episode = [false; 20];
        } else {
            if !self.observed {
                self.complete = false;
            }
            for change in &statistics.penalties {
                self.penalty(change);
            }
            self.opponent_restarts += usize::from(statistics.opponent_restart);
        }
        self.observed = true;
        for (seen, &code) in self.seen_penalty.iter_mut().zip(&self.codes) {
            *seen |= is_penalty(code) == Some(true);
        }
        self.peak_penalties = self.peak_penalties.max(
            self.codes
                .iter()
                .filter(|&&code| is_penalty(code) == Some(true))
                .count(),
        );
    }

    fn penalty(&mut self, change: &PenaltyChange) {
        let Some(player) = change
            .player
            .checked_sub(1)
            .map(usize::from)
            .filter(|&p| p < 20)
        else {
            self.complete = false;
            return;
        };
        let (Some(before), Some(after)) = (is_penalty(change.before), is_penalty(change.after))
        else {
            self.complete = false;
            self.codes[player] = change.after;
            self.counted_episode[player] = false;
            return;
        };
        let was_active = is_penalty(self.codes[player]) == Some(true);
        if after && was_active && self.counted_episode[player] {
            if self.codes[player] == 5 && change.after != 5 {
                self.penalties[player].incapable -= 1;
            } else if self.codes[player] != 5 && change.after == 5 {
                self.penalties[player].incapable += 1;
            }
        } else if after && !was_active && !before {
            self.penalties[player].total += 1;
            self.penalties[player].incapable += usize::from(change.after == 5);
            self.counted_episode[player] = true;
        } else if !after {
            self.counted_episode[player] = false;
        }
        self.codes[player] = change.after;
    }
}

pub(crate) fn markdown(session: &Session) -> Option<String> {
    let aggregate = Aggregate::from_session(session);
    if !aggregate.observed || !aggregate.ended {
        return None;
    }
    let (home, away) = session.raw_score_for(MatchType::Official);
    let tracked = markdown_text(session.metadata.tracked_team_name());
    let goals = if session.metadata.tracked_are_home() {
        home
    } else {
        away
    };
    let complete = aggregate.complete && home >= 0 && away >= 0;
    let mut output = if complete {
        "\n### Official match statistics\n\n".to_owned()
    } else {
        "\n### Official match statistics (partial GC coverage)\n\n".to_owned()
    };
    output.push_str("- Penalties observed (total/incapable): ");
    let robots = aggregate
        .penalties
        .iter()
        .enumerate()
        .filter_map(|(index, penalties)| {
            let label = session
                .metadata
                .robots
                .get(index)
                .map(String::as_str)
                .unwrap_or("");
            if label.trim().is_empty() && !aggregate.seen_penalty[index] {
                return None;
            }
            let label = if label.trim().is_empty() {
                String::new()
            } else {
                format!(" {}", markdown_text(label))
            };
            Some(format!(
                "#{}{label} {}/{}",
                index + 1,
                penalties.total,
                penalties.incapable
            ))
        })
        .collect::<Vec<_>>();
    if aggregate
        .penalties
        .iter()
        .all(|penalties| penalties.total == 0)
    {
        output.push_str("none observed");
        if !robots.is_empty() {
            output.push_str("; ");
        }
    }
    output.push_str(&robots.join("; "));
    output.push('\n');
    let goals = goals.max(0) as u64;
    let denominator = goals + aggregate.opponent_restarts as u64;
    let share = if !complete {
        "unavailable".to_owned()
    } else if denominator == 0 {
        "—".to_owned()
    } else {
        let percent = 100.0 * goals as f64 / denominator as f64;
        if percent.fract() == 0.0 {
            format!("{percent:.0}%")
        } else {
            format!("{percent:.1}%")
        }
    };
    output.push_str(&format!(
        "- Goal share: {share} ({goals} {tracked} goals, {} observed opponent goal-kicks/kick-ins)\n",
        aggregate.opponent_restarts,
    ));
    if aggregate.peak_penalties >= 2 {
        output.push_str(&format!(
            "- Peak observed simultaneous penalties: {} {tracked}\n",
            aggregate.peak_penalties,
        ));
    } else if complete && aggregate.run_available && aggregate.longest_run >= 3 {
        output.push_str(&format!(
            "- Longest {tracked} unanswered scoring run: {} goals\n",
            aggregate.longest_run,
        ));
    }
    output.push('\n');
    Some(output)
}
