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

// Cautioned and Substitute are roster/card states, not penalty episodes.
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
                aggregate.entry(entry, session.metadata.hulks_are_home());
            }
        }
        aggregate
    }

    fn entry(&mut self, entry: &Entry, hulks_are_home: bool) {
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
            if (entry.kind == EventKind::GoalHome) == hulks_are_home {
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
        // Apply the complete packet first: simultaneous changes have no known internal order.
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
            // A reason correction reclassifies the current episode, not a second penalty.
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
        // Nonzero-to-nonzero changes never recreate an episode whose start was removed.
        self.codes[player] = change.after;
    }
}

pub(crate) fn markdown(session: &Session) -> Option<String> {
    let aggregate = Aggregate::from_session(session);
    if !aggregate.observed || !aggregate.ended {
        return None;
    }
    let (home, away) = session.raw_score_for(MatchType::Official);
    let goals = if session.metadata.hulks_are_home() {
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
        "- Goal share: {share} ({goals} HULKs goals, {} observed opponent goal-kicks/kick-ins)\n",
        aggregate.opponent_restarts,
    ));
    if aggregate.peak_penalties >= 2 {
        output.push_str(&format!(
            "- Peak observed simultaneous penalties: {} HULKs\n",
            aggregate.peak_penalties,
        ));
    } else if complete && aggregate.run_available && aggregate.longest_run >= 3 {
        output.push_str(&format!(
            "- Longest HULKs unanswered scoring run: {} goals\n",
            aggregate.longest_run,
        ));
    }
    output.push('\n');
    Some(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Metadata;
    use chrono::{DateTime, NaiveDate};

    fn empty() -> Statistics {
        Statistics {
            baseline: None,
            penalties: vec![],
            opponent_restart: false,
        }
    }

    fn baseline(pre_game: bool, penalties: [u8; 20]) -> Statistics {
        Statistics {
            baseline: Some(StatisticsBaseline {
                penalties,
                pre_game,
            }),
            ..empty()
        }
    }

    fn changes(values: &[(u8, u8, u8)]) -> Statistics {
        Statistics {
            penalties: values
                .iter()
                .map(|&(player, before, after)| PenaltyChange {
                    player,
                    before,
                    after,
                })
                .collect(),
            ..empty()
        }
    }

    fn add(session: &mut Session, kind: EventKind, statistics: Option<Statistics>) -> usize {
        session.record(
            kind,
            DateTime::parse_from_rfc3339("2026-10-04T12:00:00+02:00").unwrap(),
        );
        let index = session.entries.len() - 1;
        let entry = &mut session.entries[index];
        entry.gc_seconds_remaining = Some(0);
        entry.from_controller = statistics.is_some();
        entry.statistics = statistics;
        index
    }

    fn session() -> Session {
        let mut session = Session::new_match(
            Metadata {
                event: "RoboCup".into(),
                home: "HULKs".into(),
                away: "Other".into(),
                date: NaiveDate::from_ymd_opt(2026, 10, 4).unwrap(),
                label: "Match".into(),
                robots: [
                    "Amy".into(),
                    "Bob".into(),
                    String::new(),
                    String::new(),
                    String::new(),
                ],
            },
            MatchType::Official,
        );
        add(
            &mut session,
            EventKind::Custom,
            Some(baseline(true, [0; 20])),
        );
        session
    }

    fn end(session: &mut Session) -> usize {
        add(session, EventKind::GameEnd, Some(empty()))
    }

    #[test]
    fn episodes_deduplicate_reclassify_and_count_repeated_incapable_penalties() {
        let mut session = session();
        for change in [(1, 0, 1), (1, 0, 1), (1, 1, 5)] {
            add(&mut session, EventKind::Custom, Some(changes(&[change])));
        }
        let aggregate = Aggregate::from_session(&session);
        assert_eq!(
            (
                aggregate.penalties[0].total,
                aggregate.penalties[0].incapable
            ),
            (1, 1)
        );
        add(&mut session, EventKind::Custom, Some(changes(&[(1, 5, 1)])));
        assert_eq!(Aggregate::from_session(&session).penalties[0].incapable, 0);
        for change in [
            (1, 1, 5),
            (1, 5, 0),
            (1, 0, 5),
            (2, 0, 11),
            (2, 11, 13),
            (20, 0, 12),
        ] {
            add(&mut session, EventKind::Custom, Some(changes(&[change])));
        }
        end(&mut session);
        let aggregate = Aggregate::from_session(&session);
        assert_eq!(
            (
                aggregate.penalties[0].total,
                aggregate.penalties[0].incapable
            ),
            (2, 2)
        );
        assert_eq!(aggregate.penalties[1].total, 0);
        assert_eq!(aggregate.penalties[19].total, 1);
        let text = markdown(&session).unwrap();
        assert!(text.contains("#1 Amy 2/2; #2 Bob 0/0; #20 1/0"));
        assert!(!text.contains("#3"));
    }

    #[test]
    fn baselines_observe_overlap_without_inventing_episodes_or_restarts() {
        let mut session = session();
        let mut codes = [0; 20];
        codes[0] = 1;
        codes[19] = 5;
        let mut snapshot = baseline(false, codes);
        snapshot.opponent_restart = true;
        snapshot.penalties = changes(&[(2, 0, 5)]).penalties;
        session.entries[0].statistics = Some(snapshot);
        add(&mut session, EventKind::Custom, Some(changes(&[(1, 1, 5)])));
        add(
            &mut session,
            EventKind::Custom,
            Some(baseline(false, codes)),
        );
        end(&mut session);
        let aggregate = Aggregate::from_session(&session);
        assert!(aggregate.penalties.iter().all(|p| p.total == 0));
        assert_eq!(aggregate.opponent_restarts, 0);
        assert_eq!(aggregate.peak_penalties, 2);
        assert!(!aggregate.complete);
        let text = markdown(&session).unwrap();
        assert!(text.contains("none observed"));
        assert!(text.contains("#20 0/0"));
        assert!(text.contains("Peak observed simultaneous penalties: 2 HULKs"));
        assert!(text.contains("Goal share: unavailable"));
        add(
            &mut session,
            EventKind::Custom,
            Some(changes(&[(1, 1, 0), (1, 0, 5)])),
        );
        assert_eq!(Aggregate::from_session(&session).penalties[0].total, 1);
    }

    #[test]
    fn packet_changes_are_atomic_for_overlap() {
        let mut session = session();
        add(&mut session, EventKind::Custom, Some(changes(&[(1, 0, 1)])));
        add(
            &mut session,
            EventKind::Custom,
            Some(changes(&[(2, 0, 5), (1, 1, 0)])),
        );
        end(&mut session);
        assert_eq!(Aggregate::from_session(&session).peak_penalties, 1);
        assert!(!markdown(&session).unwrap().contains("Peak observed"));
    }

    #[test]
    fn removing_and_restoring_events_replays_counts_restarts_overlap_and_json() {
        let mut session = session();
        let penalty = add(&mut session, EventKind::Custom, Some(changes(&[(1, 0, 5)])));
        add(&mut session, EventKind::Custom, Some(changes(&[(2, 0, 1)])));
        let restart = add(
            &mut session,
            EventKind::Custom,
            Some(Statistics {
                opponent_restart: true,
                ..empty()
            }),
        );
        add(&mut session, EventKind::Custom, Some(empty()));
        add(&mut session, EventKind::GoalHome, Some(empty()));
        let ending = end(&mut session);
        let original = session.markdown();
        assert!(original.contains("Goal share: 50%"));
        assert!(original.contains("Peak observed simultaneous penalties: 2"));
        session.delete(penalty);
        session.delete(restart);
        let edited = session.markdown();
        assert!(edited.contains("#1 Amy 0/0; #2 Bob 1/0"));
        assert!(edited.contains("Goal share: 100%"));
        assert!(!edited.contains("Peak observed"));
        let mut loaded: Session =
            serde_json::from_str(&serde_json::to_string(&session).unwrap()).unwrap();
        assert_eq!(loaded, session);
        assert_eq!(loaded.markdown(), edited);
        loaded.restore(penalty);
        loaded.restore(restart);
        assert_eq!(loaded.markdown(), original);
        loaded.delete(ending);
        assert!(!loaded.markdown().contains("Official match statistics"));
        add(&mut loaded, EventKind::GoalHome, Some(empty()));
        end(&mut loaded);
        loaded.restore(ending);
        let restored = loaded.markdown();
        assert_eq!(restored.matches("Official match statistics").count(), 1);
        assert!(restored.contains("Goal share: 66.7% (2 HULKs goals"));
    }

    #[test]
    fn reason_change_does_not_recreate_a_removed_episode_start() {
        let mut session = session();
        let start = add(&mut session, EventKind::Custom, Some(changes(&[(1, 0, 1)])));
        let reason = add(&mut session, EventKind::Custom, Some(changes(&[(1, 1, 5)])));
        end(&mut session);
        session.delete(reason);
        assert_eq!(Aggregate::from_session(&session).penalties[0].incapable, 0);
        session.restore(reason);
        session.delete(start);
        let aggregate = Aggregate::from_session(&session);
        assert_eq!(
            (
                aggregate.penalties[0].total,
                aggregate.penalties[0].incapable
            ),
            (0, 0)
        );
        session.restore(start);
        let aggregate = Aggregate::from_session(&session);
        assert_eq!(
            (
                aggregate.penalties[0].total,
                aggregate.penalties[0].incapable
            ),
            (1, 1)
        );
    }

    #[test]
    fn goal_share_uses_hulks_side_and_handles_empty_denominators() {
        let mut session = session();
        std::mem::swap(&mut session.metadata.home, &mut session.metadata.away);
        end(&mut session);
        assert!(
            markdown(&session)
                .unwrap()
                .contains("Goal share: — (0 HULKs goals, 0 observed")
        );
        add(
            &mut session,
            EventKind::Custom,
            Some(Statistics {
                opponent_restart: true,
                ..empty()
            }),
        );
        assert!(markdown(&session).unwrap().contains("Goal share: 0%"));
        add(&mut session, EventKind::GoalHome, Some(empty()));
        assert!(markdown(&session).unwrap().contains("Goal share: 0%"));
        add(&mut session, EventKind::GoalAway, Some(empty()));
        assert!(
            markdown(&session)
                .unwrap()
                .contains("Goal share: 50% (1 HULKs goals, 1 observed")
        );
    }

    #[test]
    fn incomplete_coverage_never_divides_full_score_by_partial_restarts() {
        let original = session();
        for missing in 0..9 {
            let mut session = original.clone();
            match missing {
                0 => session.entries[0].statistics = Some(empty()),
                1 => {
                    session.entries[0]
                        .statistics
                        .as_mut()
                        .unwrap()
                        .baseline
                        .as_mut()
                        .unwrap()
                        .pre_game = false
                }
                2 => {
                    add(
                        &mut session,
                        EventKind::Custom,
                        Some(baseline(true, [0; 20])),
                    );
                }
                3 => {
                    session.entries[0]
                        .statistics
                        .as_mut()
                        .unwrap()
                        .baseline
                        .as_mut()
                        .unwrap()
                        .penalties[0] = 5
                }
                4 => {
                    let index = add(&mut session, EventKind::Custom, None);
                    session.entries[index].from_controller = true;
                }
                5 => {
                    let index = end(&mut session);
                    session.entries[index].gc_seconds_remaining = None;
                }
                6 => session.entries[0].score_delta = Some([2, 0]),
                7 => session.delete(0),
                8 => {
                    add(&mut session, EventKind::GoalHome, None);
                    session.entries.swap(0, 1);
                }
                _ => unreachable!(),
            }
            add(
                &mut session,
                EventKind::Custom,
                Some(Statistics {
                    opponent_restart: true,
                    ..empty()
                }),
            );
            add(&mut session, EventKind::GoalHome, Some(empty()));
            end(&mut session);
            let text = markdown(&session).unwrap();
            assert!(text.contains("partial GC coverage"), "case {missing}");
            assert!(text.contains("Goal share: unavailable"), "case {missing}");
            assert!(!text.contains('%'), "case {missing}");
        }
    }

    #[test]
    fn unknown_codes_make_coverage_partial_without_counting_fake_penalties() {
        let mut session = session();
        add(
            &mut session,
            EventKind::Custom,
            Some(changes(&[(1, 0, 200), (2, 201, 5)])),
        );
        end(&mut session);
        let aggregate = Aggregate::from_session(&session);
        assert!(aggregate.penalties.iter().all(|p| p.total == 0));
        assert!(
            markdown(&session)
                .unwrap()
                .contains("Goal share: unavailable")
        );
    }

    #[test]
    fn only_one_highlight_prefers_overlap_and_runs_recompute_after_goal_removal() {
        let mut session = session();
        for _ in 0..2 {
            add(&mut session, EventKind::GoalHome, Some(empty()));
        }
        let opponent_goal = add(&mut session, EventKind::GoalAway, Some(empty()));
        for _ in 0..2 {
            add(&mut session, EventKind::GoalHome, Some(empty()));
        }
        end(&mut session);
        assert!(!markdown(&session).unwrap().contains("scoring run"));
        session.delete(opponent_goal);
        assert!(markdown(&session).unwrap().contains("scoring run: 4 goals"));
        let overlap = add(
            &mut session,
            EventKind::Custom,
            Some(changes(&[(1, 0, 5), (2, 0, 1)])),
        );
        let text = markdown(&session).unwrap();
        assert!(text.contains("Peak observed simultaneous penalties: 2"));
        assert!(!text.contains("scoring run"));
        assert_eq!(
            text.lines().filter(|line| line.starts_with("- ")).count(),
            3
        );
        session.delete(overlap);
        assert!(markdown(&session).unwrap().contains("scoring run: 4 goals"));
        session.restore(opponent_goal);
        assert!(!markdown(&session).unwrap().contains("scoring run"));
    }

    #[test]
    fn ambiguous_corrections_and_unknown_goal_timing_suppress_runs() {
        let mut original = session();
        for _ in 0..3 {
            add(&mut original, EventKind::GoalHome, Some(empty()));
        }
        end(&mut original);
        assert!(
            markdown(&original)
                .unwrap()
                .contains("scoring run: 3 goals")
        );
        for delta in [[-1, 0], [1, 0], [1, -1], [-10, 0]] {
            let mut session = original.clone();
            let index = add(&mut session, EventKind::Custom, Some(empty()));
            session.entries[index].score_delta = Some(delta);
            assert!(!markdown(&session).unwrap().contains("scoring run"));
        }
        original.entries[1].gc_seconds_remaining = None;
        assert!(!markdown(&original).unwrap().contains("scoring run"));
    }

    #[test]
    fn combined_statistics_ignore_golden_goal_payloads_and_scores() {
        let mut session = session();
        session.match_type = Some(MatchType::Combined);
        session.entries[0].game = Some(MatchType::Official);
        add(&mut session, EventKind::GoalHome, Some(empty()));
        end(&mut session);
        let golden = add(
            &mut session,
            EventKind::GoalHome,
            Some(changes(&[(1, 0, 5), (2, 0, 5)])),
        );
        session.entries[golden].game = Some(MatchType::GoldenGoal);
        let golden_end = end(&mut session);
        session.entries[golden_end].game = Some(MatchType::GoldenGoal);
        let text = session.markdown();
        assert_eq!(text.matches("Official match statistics").count(), 1);
        assert!(text.contains("Goal share: 100% (1 HULKs goals"));
        assert!(text.contains("none observed"));
        assert!(!text.contains("Peak observed"));
        assert!(
            text.find("Official match statistics").unwrap()
                > text.find("## Official match").unwrap()
        );
        session.match_type = Some(MatchType::GoldenGoal);
        assert!(markdown(&session).is_none());
        assert!(!session.markdown().contains("Official match statistics"));
    }

    #[test]
    fn legacy_phase_routing_keeps_manual_test_goals_out_of_official_stats() {
        let mut session = session();
        session.match_type = None;
        session.entries.clear();
        add(&mut session, EventKind::GoldenGoalStart, None);
        for _ in 0..3 {
            add(&mut session, EventKind::GoalHome, None);
        }
        // This official pre-game sync is imported before the legacy test phase is closed.
        add(
            &mut session,
            EventKind::Custom,
            Some(baseline(true, [0; 20])),
        );
        add(&mut session, EventKind::GameStart, Some(empty()));
        add(&mut session, EventKind::GoalHome, Some(empty()));
        end(&mut session);
        let text = markdown(&session).unwrap();
        assert!(text.contains("Goal share: 100% (1 HULKs goals"));
        assert!(!text.contains("scoring run"));
        assert!(!text.contains("partial GC coverage"));
    }

    #[test]
    fn no_payloads_preserve_exports_and_short_summary_escapes_robot_labels() {
        let mut session = session();
        end(&mut session);
        session.metadata.robots[0] = "[Amy]*".into();
        let text = markdown(&session).unwrap();
        assert!(text.contains("#1 \\[Amy\\]\\* 0/0"));
        assert_eq!(
            text.lines().filter(|line| line.starts_with("- ")).count(),
            2
        );
        for entry in &mut session.entries {
            entry.statistics = None;
        }
        assert!(markdown(&session).is_none());
        let json = serde_json::to_value(&session).unwrap();
        assert!(
            json["entries"]
                .as_array()
                .unwrap()
                .iter()
                .all(|entry| entry.get("statistics").is_none())
        );
        let mut explicit_null = json.clone();
        for entry in explicit_null["entries"].as_array_mut().unwrap() {
            entry["statistics"] = serde_json::Value::Null;
        }
        let loaded: Session = serde_json::from_value(json).unwrap();
        let loaded_null: Session = serde_json::from_value(explicit_null).unwrap();
        assert_eq!(session.markdown(), loaded.markdown());
        assert_eq!(session.markdown(), loaded_null.markdown());
        assert!(!session.markdown().contains("Official match statistics"));
    }

    #[test]
    fn validation_rejects_invalid_players_and_golden_payloads() {
        for player in [0, 21, 255] {
            let mut session = session();
            add(
                &mut session,
                EventKind::Custom,
                Some(changes(&[(player, 0, 5)])),
            );
            assert!(session.validate().unwrap_err().contains("player number"));
            end(&mut session);
            assert!(
                markdown(&session)
                    .unwrap()
                    .contains("Goal share: unavailable")
            );
        }
        let mut session = session();
        assert!(session.validate().is_ok());
        session.match_type = Some(MatchType::GoldenGoal);
        session.metadata.away = "Invisibles".into();
        assert!(session.validate().unwrap_err().contains("official match"));
    }
}
