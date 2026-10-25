use crate::{
    app::{App, Mode},
    game_controller::{Snapshot, Update, decode_capture},
    model::{EventKind, MatchType},
    statistics::Statistics,
};

impl App {
    pub fn poll_controller(&mut self) {
        if let Some(controller) = &mut self.controller {
            controller.allow_opponent_change = self.session.as_ref().is_some_and(|session| {
                session.match_type == Some(MatchType::Combined)
                    && session.current_game() == MatchType::GoldenGoal
                    && session.is_finished()
                    && !self.pending_controller.iter().any(|update| {
                        session
                            .golden_goal_opponent
                            .is_some_and(|opponent| update.snapshot.starts_official_after(opponent))
                    })
            });
            controller.allow_same_opponent_restart = self.session.as_ref().and_then(|session| {
                controller
                    .allow_opponent_change
                    .then_some(session.golden_goal_opponent)
                    .flatten()
            });
            controller.ignored_opponent = self.session.as_ref().and_then(|session| {
                let official_opponent = session
                    .official_opponent
                    .or_else(|| crate::teams::team_number(session.metadata.opponent_name()));
                (session.match_type == Some(MatchType::Combined)
                    && session.current_game() == MatchType::Official
                    && official_opponent != session.golden_goal_opponent)
                    .then_some(session.golden_goal_opponent)
                    .flatten()
            });
            let previous_receipt = controller.received_at;
            let previously_pending = self.pending_controller.len();
            match controller.poll() {
                Ok(updates) => {
                    if controller.received_at != previous_receipt {
                        self.controller_error = None;
                    }
                    self.pending_controller.extend(updates);
                }
                Err(error) => {
                    self.controller_error = Some(format!(
                        "GC receive failed: {error}; Actions → Pause GC logging"
                    ))
                }
            }
            let mut captures = std::mem::take(&mut controller.captures);
            if let Some(session) = &self.session {
                let mut game = session.current_game();
                let mut finished = session.is_finished();
                let mut test_opponent = session.golden_goal_opponent;
                let mut capture_context = |snapshot: Option<&Snapshot>| {
                    if session.match_type == Some(MatchType::Combined)
                        && game == MatchType::GoldenGoal
                        && let Some(snapshot) = snapshot
                    {
                        if finished
                            && test_opponent
                                .is_some_and(|opponent| snapshot.starts_official_after(opponent))
                        {
                            game = MatchType::Official;
                            finished = false;
                        } else if test_opponent.is_none() {
                            test_opponent = Some(snapshot.opponent_number);
                        }
                    }
                    let context = (Some(game), !finished);
                    if let Some(snapshot) = snapshot
                        && matches!(snapshot.game_phase, 0 | 2)
                        && snapshot.state == 4
                        && (game == MatchType::GoldenGoal || !snapshot.first_half)
                    {
                        finished = true;
                    }
                    context
                };
                for update in self.pending_controller.iter().take(previously_pending) {
                    capture_context(Some(&update.snapshot));
                }
                for packet in &mut captures {
                    let snapshot = if packet.accepted {
                        decode_capture(
                            &packet.packet,
                            packet.true_data,
                            session.metadata.tracked_team_number(),
                        )
                        .ok()
                        .flatten()
                    } else {
                        None
                    };
                    (packet.game, packet.active) = capture_context(snapshot.as_ref());
                }
            }
            if !captures.is_empty() {
                self.last_capture_id = controller.next_capture_id().checked_sub(1);
            }
            self.pending_capture.extend(captures);
        }
        if !matches!(self.mode, Mode::Actions { .. }) {
            self.flush_controller();
        }
    }

    pub fn flush_controller(&mut self) {
        for update in std::mem::take(&mut self.pending_controller) {
            self.import_controller(update);
        }
    }

    fn import_controller(&mut self, update: Update) {
        let Some(session) = &mut self.session else {
            return;
        };
        let snapshot = &update.snapshot;
        let combined = session.match_type == Some(MatchType::Combined);
        let current_game = session.current_game();
        if combined
            && current_game == MatchType::GoldenGoal
            && session.golden_goal_opponent.is_none()
        {
            session.golden_goal_opponent = Some(snapshot.opponent_number);
            self.dirty = true;
        }
        let opponent_changed =
            combined && session.golden_goal_opponent != Some(snapshot.opponent_number);
        let transitioning = combined
            && current_game == MatchType::GoldenGoal
            && session.is_finished()
            && session
                .golden_goal_opponent
                .is_some_and(|opponent| snapshot.starts_official_after(opponent));
        if combined
            && ((current_game == MatchType::GoldenGoal && opponent_changed && !transitioning)
                || (current_game == MatchType::Official
                    && !opponent_changed
                    && session
                        .official_opponent
                        .or_else(|| crate::teams::team_number(session.metadata.opponent_name()))
                        != session.golden_goal_opponent))
        {
            return;
        }
        let previous = if transitioning {
            None
        } else {
            update.previous.as_ref()
        };
        let animation_time = std::time::Instant::now();
        self.arcade
            .observe_controller(session, previous, snapshot, animation_time);
        if session.match_type.is_some() && session.is_finished() && !transitioning {
            return;
        }
        let target_game = if transitioning {
            MatchType::Official
        } else {
            current_game
        };
        let fixed_golden_goal = session.match_type == Some(MatchType::GoldenGoal)
            || (combined && target_game == MatchType::GoldenGoal);
        if previous.is_some_and(|packet| {
            snapshot.same_game_state(packet)
                || (target_game == MatchType::GoldenGoal && snapshot.same_match_state(packet))
        }) {
            return;
        }
        let golden_goal_message_budget = if session.match_type.is_none() && session.in_golden_goal()
        {
            previous.map(|packet| packet.tracked_message_budget)
        } else {
            None
        };
        let official = matches!(snapshot.game_phase, 0 | 2);
        let sync = previous.is_none()
            || (official && previous.is_some_and(|p| p.game_phase != snapshot.game_phase));
        let oriented = |packet: &Snapshot| {
            if session.metadata.tracked_are_home() {
                (
                    i64::from(packet.tracked_score),
                    i64::from(packet.opponent_score),
                )
            } else {
                (
                    i64::from(packet.opponent_score),
                    i64::from(packet.tracked_score),
                )
            }
        };
        let target = oriented(snapshot);
        let old_score = if sync {
            if combined {
                session.raw_score_for(target_game)
            } else {
                session.raw_score()
            }
        } else {
            oriented(previous.unwrap())
        };
        let mut delta = [0_i32, 0_i32];
        if official {
            let (Ok(home), Ok(away)) = (
                i32::try_from(target.0 - old_score.0),
                i32::try_from(target.1 - old_score.1),
            ) else {
                self.controller_error =
                    Some("Logged score too large to synchronize with GC".into());
                return;
            };
            delta = [home, away];
        }
        let phase_changed = sync
            || previous
                .is_some_and(|p| p.state != snapshot.state || p.first_half != snapshot.first_half);
        let mut phase = if official && phase_changed {
            match (snapshot.state, snapshot.first_half, fixed_golden_goal) {
                (3, _, true) => Some(EventKind::GoldenGoalStart),
                (4, _, true) => Some(EventKind::GameEnd),
                (3, true, false) => Some(EventKind::GameStart),
                (3, false, false) => Some(EventKind::SecondHalfStart),
                (4, true, false) => Some(EventKind::HalfTime),
                (4, false, false) => Some(EventKind::GameEnd),
                _ => None,
            }
        } else {
            None
        };
        if phase.is_none()
            && session.match_type.is_none()
            && official
            && !sync
            && delta.iter().any(|change| *change > 0)
            && session.in_golden_goal()
        {
            phase = Some(if snapshot.first_half {
                EventKind::GameStart
            } else {
                EventKind::SecondHalfStart
            });
        }
        let last_phase = session
            .entries
            .iter()
            .rev()
            .filter(|entry| !entry.undone && (!combined || entry.game == Some(target_game)))
            .find_map(|entry| {
                matches!(
                    entry.kind,
                    EventKind::GoldenGoalStart
                        | EventKind::GameStart
                        | EventKind::HalfTime
                        | EventKind::SecondHalfStart
                        | EventKind::GameEnd
                )
                .then_some(entry.kind)
            });
        if phase == last_phase && (combined || fixed_golden_goal || !session.in_golden_goal()) {
            phase = None;
        }
        let kind = phase.unwrap_or_else(|| {
            if official && !sync && (combined || fixed_golden_goal || !session.in_golden_goal()) {
                match delta {
                    [1, 0] => EventKind::GoalHome,
                    [0, 1] => EventKind::GoalAway,
                    _ => EventKind::Custom,
                }
            } else {
                EventKind::Custom
            }
        });
        let leaving_legacy_test = session.match_type.is_none()
            && current_game == MatchType::GoldenGoal
            && matches!(
                kind,
                EventKind::GameStart
                    | EventKind::SecondHalfStart
                    | EventKind::HalfTime
                    | EventKind::GameEnd
            );
        let statistics = (matches!(snapshot.game_phase, 0 | 2 | 3)
            && (target_game == MatchType::Official || leaving_legacy_test))
            .then(|| {
                let previous = previous.filter(|packet| {
                    !leaving_legacy_test
                        && matches!(packet.game_phase, 0 | 2 | 3)
                        && packet.opponent_number == snapshot.opponent_number
                });
                Statistics::from_snapshots(snapshot, previous)
            });
        let follow = matches!(self.mode, Mode::Idle | Mode::Writing)
            && self.history.selected() == session.entries.len().checked_sub(1);
        if target_game == MatchType::Official
            && (transitioning || session.official_opponent.is_none())
        {
            session.official_opponent = Some(snapshot.opponent_number);
        }
        let (home, away) = session.teams_for(target_game);
        let (home, away) = (home.into_owned(), away.into_owned());
        session.record(kind, update.received_at);
        let entry = session.entries.last_mut().unwrap();
        if combined {
            entry.game = Some(target_game);
        }
        entry.capture_id = update.capture_id;
        entry.from_controller = true;
        entry.gc_seconds_remaining = Some(snapshot.seconds_remaining);
        entry.message_budget = Some(snapshot.tracked_message_budget);
        entry.golden_goal_message_budget = golden_goal_message_budget;
        entry.statistics = statistics;
        if !matches!(kind, EventKind::GoalHome | EventKind::GoalAway) && delta != [0, 0] {
            entry.score_delta = Some(delta);
        }
        let penalty_description = entry
            .statistics
            .as_ref()
            .filter(|statistics| !statistics.penalties.is_empty())
            .map(|statistics| {
                format!(
                    "{} penalties: {}; ",
                    session.metadata.tracked_team_name(),
                    statistics
                        .penalties
                        .iter()
                        .map(|change| format!(
                            "player {} {}→{}",
                            change.player, change.before, change.after
                        ))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })
            .unwrap_or_default();
        entry.text = format!(
            "GC {}received: {} {}:{} {}; opponent ID {}; {}{}",
            if sync { "sync — " } else { "" },
            home,
            target.0,
            target.1,
            away,
            snapshot.opponent_number,
            penalty_description,
            snapshot.summary()
        );
        if sync && phase.is_some() {
            entry
                .text
                .push_str("; phase observed on attachment, not the original transition time");
        }
        self.arcade.observe(session, animation_time);
        self.dirty = true;
        if follow {
            self.history.select(Some(session.entries.len() - 1));
        }
    }
}
