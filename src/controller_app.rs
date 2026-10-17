use crate::{
    app::{App, Mode},
    game_controller::{Snapshot, Update, decode_capture},
    model::{EventKind, MatchType},
    statistics::{PenaltyChange, Statistics, StatisticsBaseline},
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
                            .is_some_and(|opponent| update.snapshot.opponent_number != opponent)
                    })
            });
            controller.ignored_opponent = self.session.as_ref().and_then(|session| {
                (session.match_type == Some(MatchType::Combined)
                    && session.current_game() == MatchType::Official)
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
                        "GC receive failed: {error}; Tab, L stops listening"
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
                                .is_some_and(|opponent| opponent != snapshot.opponent_number)
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
                // The menu delays history imports, not receipt-time game boundaries.
                for update in self.pending_controller.iter().take(previously_pending) {
                    capture_context(Some(&update.snapshot));
                }
                for packet in &mut captures {
                    let snapshot = if packet.accepted {
                        decode_capture(&packet.packet, packet.true_data)
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
        // Tab captured an earlier moment: apply its event before later controller events.
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
            && opponent_changed;
        if combined
            && ((current_game == MatchType::GoldenGoal && opponent_changed && !transitioning)
                || (current_game == MatchType::Official && !opponent_changed))
        {
            return;
        }
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
        let previous = if transitioning {
            None
        } else {
            update.previous.as_ref()
        };
        if target_game == MatchType::GoldenGoal
            && previous.is_some_and(|packet| snapshot.same_match_state(packet))
        {
            return;
        }
        let golden_goal_message_budget = if session.match_type.is_none() && session.in_golden_goal()
        {
            previous.map(|packet| packet.hulks_message_budget)
        } else {
            None
        };
        let official = matches!(snapshot.game_phase, 0 | 2);
        let sync = previous.is_none()
            || (official && previous.is_some_and(|p| p.game_phase != snapshot.game_phase));
        let oriented = |packet: &Snapshot| {
            if session.metadata.hulks_are_home() {
                (
                    i64::from(packet.hulks_score),
                    i64::from(packet.opponent_score),
                )
            } else {
                (
                    i64::from(packet.opponent_score),
                    i64::from(packet.hulks_score),
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
        // A reported official goal must not be attributed to an unfinished deployment test.
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
                // A test/phase/source handoff supplies a baseline, not observed events.
                let previous = previous.filter(|packet| {
                    !leaving_legacy_test
                        && matches!(packet.game_phase, 0 | 2 | 3)
                        && packet.opponent_number == snapshot.opponent_number
                });
                Statistics {
                    baseline: previous.is_none().then_some(StatisticsBaseline {
                        penalties: snapshot.hulks_penalties,
                        pre_game: snapshot.game_phase == 0
                            && snapshot.first_half
                            && snapshot.state == 0
                            && snapshot.hulks_score == 0
                            && snapshot.opponent_score == 0,
                    }),
                    penalties: previous.map_or_else(Vec::new, |packet| {
                        packet
                            .hulks_penalties
                            .iter()
                            .zip(snapshot.hulks_penalties)
                            .enumerate()
                            .filter_map(|(player, (&before, after))| {
                                (before != after).then_some(PenaltyChange {
                                    player: player as u8 + 1,
                                    before,
                                    after,
                                })
                            })
                            .collect()
                    }),
                    opponent_restart: previous.is_some_and(|packet| {
                        official
                            && matches!(snapshot.set_play, 4 | 5)
                            && snapshot.kicking_team == snapshot.opponent_number
                            && (packet.set_play, packet.kicking_team)
                                != (snapshot.set_play, snapshot.kicking_team)
                    }),
                }
            });
        let follow = matches!(self.mode, Mode::Idle | Mode::Writing)
            && self.history.selected() == session.entries.len().checked_sub(1);
        session.record(kind, update.received_at);
        let entry = session.entries.last_mut().unwrap();
        if combined {
            entry.game = Some(target_game);
        }
        entry.capture_id = update.capture_id;
        entry.from_controller = true;
        entry.gc_seconds_remaining = Some(snapshot.seconds_remaining);
        entry.message_budget = Some(snapshot.hulks_message_budget);
        entry.golden_goal_message_budget = golden_goal_message_budget;
        entry.statistics = statistics;
        if !matches!(kind, EventKind::GoalHome | EventKind::GoalAway) && delta != [0, 0] {
            entry.score_delta = Some(delta);
        }
        let (home, away) = if combined && target_game == MatchType::GoldenGoal {
            session.metadata.golden_goal_teams()
        } else {
            (
                session.metadata.home.as_str(),
                session.metadata.away.as_str(),
            )
        };
        let penalty_description = entry
            .statistics
            .as_ref()
            .filter(|statistics| !statistics.penalties.is_empty())
            .map(|statistics| {
                format!(
                    "HULKs penalties: {}; ",
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
        self.dirty = true;
        if follow {
            self.history.select(Some(session.entries.len() - 1));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        game_controller::Receiver,
        model::{Metadata, Session},
    };
    use chrono::{DateTime, FixedOffset};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn time() -> DateTime<FixedOffset> {
        DateTime::parse_from_rfc3339("2026-10-03T12:00:00+02:00").unwrap()
    }

    fn app() -> App {
        let mut app = App::new(None).unwrap();
        app.session = Some(Session::new(Metadata {
            event: "Hobbymesse".into(),
            home: "HULKs".into(),
            away: "HTWK".into(),
            date: time().date_naive(),
            label: "game".into(),
            robots: std::array::from_fn(|_| String::new()),
        }));
        app.mode = Mode::Idle;
        app
    }

    fn fixed_app(match_type: MatchType) -> App {
        let mut app = app();
        let mut metadata = app.session.take().unwrap().metadata;
        if match_type == MatchType::GoldenGoal {
            metadata.away = "Invisibles".into();
        }
        app.session = Some(Session::new_match(metadata, match_type));
        app
    }

    fn snapshot(score: u8) -> Snapshot {
        Snapshot {
            packet_number: score,
            game_phase: 0,
            state: 3,
            set_play: 0,
            first_half: true,
            stopped: false,
            kicking_team: 24,
            seconds_remaining: 540,
            secondary_time: 0,
            hulks_score: score,
            hulks_message_budget: 1200,
            hulks_penalties: [0; 20],
            opponent_score: 0,
            opponent_number: 4,
        }
    }

    fn import(app: &mut App, current: Snapshot, previous: Option<Snapshot>) {
        app.import_controller(Update {
            capture_id: None,
            snapshot: current,
            previous,
            received_at: time(),
        });
    }

    fn press(app: &mut App, code: KeyCode) {
        app.key(KeyEvent::new(code, KeyModifiers::NONE), time())
            .unwrap();
    }

    #[test]
    fn combined_switches_only_after_test_end_and_new_opponent_ready_or_playing() {
        for next_state in [1, 3] {
            let mut app = fixed_app(MatchType::Combined);
            let start = snapshot(2);
            import(&mut app, start.clone(), None);
            let mut next = snapshot(0);
            next.opponent_number = 9;
            next.state = next_state;
            next.hulks_penalties[0] = 2;
            next.set_play = 4;
            next.kicking_team = 9;
            import(&mut app, next.clone(), None);
            assert_eq!(app.session.as_ref().unwrap().entries.len(), 1);
            let mut ended = start.clone();
            ended.state = 4;
            import(&mut app, ended.clone(), Some(start));
            let frozen = app.session.as_ref().unwrap().clone();
            assert_eq!(frozen.current_game(), MatchType::GoldenGoal);
            assert!(frozen.is_finished());
            assert_eq!(frozen.golden_goal_opponent, Some(4));
            import(&mut app, snapshot(0), None);
            assert_eq!(*app.session.as_ref().unwrap(), frozen);
            import(&mut app, next.clone(), Some(ended));
            let session = app.session.as_ref().unwrap();
            assert_eq!(session.current_game(), MatchType::Official);
            assert!(!session.is_finished());
            assert_eq!(session.raw_score_for(MatchType::GoldenGoal), (2, 0));
            assert_eq!(session.raw_score_for(MatchType::Official), (0, 0));
            assert_eq!(session.entries[2].game, Some(MatchType::Official));
            assert_eq!(session.entries[2].score_delta, None);
            assert!(
                session.entries[..2]
                    .iter()
                    .all(|entry| entry.statistics.is_none())
            );
            let statistics = session.entries[2].statistics.as_ref().unwrap();
            assert_eq!(
                statistics.baseline.as_ref().unwrap().penalties,
                next.hulks_penalties
            );
            assert!(!statistics.baseline.as_ref().unwrap().pre_game);
            assert!(statistics.penalties.is_empty());
            assert!(!statistics.opponent_restart);
            assert_eq!(
                session.entries[2].kind,
                if next_state == 1 {
                    EventKind::Custom
                } else {
                    EventKind::GameStart
                }
            );
            assert!(session.entries[2].text.starts_with("GC sync"));
            assert_eq!(session.metadata.away, "HTWK");
            assert!(session.entries[0].text.contains("Invisibles"));
            assert!(session.entries[2].text.contains("HTWK"));
            if next_state == 1 {
                let mut playing = next.clone();
                playing.state = 3;
                import(&mut app, playing, Some(next));
                assert_eq!(
                    app.session.as_ref().unwrap().entries[3].kind,
                    EventKind::GameStart
                );
            }
        }
    }

    #[test]
    fn combined_games_keep_independent_scores_budgets_and_clocks() {
        let mut app = fixed_app(MatchType::Combined);
        let mut test = snapshot(1);
        test.seconds_remaining = 42;
        import(&mut app, test.clone(), None);
        let mut test_end = snapshot(2);
        test_end.state = 4;
        test_end.seconds_remaining = -3;
        test_end.hulks_message_budget = 17;
        import(&mut app, test_end.clone(), Some(test));
        let mut official = snapshot(0);
        official.opponent_number = 9;
        official.state = 1;
        official.seconds_remaining = 600;
        import(&mut app, official.clone(), Some(test_end));
        let mut previous = official.clone();
        official.state = 3;
        official.seconds_remaining = 540;
        import(&mut app, official.clone(), Some(previous));
        previous = official.clone();
        official.hulks_score = 1;
        official.seconds_remaining = 400;
        import(&mut app, official.clone(), Some(previous));
        previous = official.clone();
        official.state = 4;
        official.hulks_message_budget = 700;
        import(&mut app, official.clone(), Some(previous));
        previous = official.clone();
        official.state = 3;
        official.first_half = false;
        official.seconds_remaining = 600;
        import(&mut app, official.clone(), Some(previous));
        previous = official.clone();
        official.state = 4;
        official.hulks_score = 3;
        official.seconds_remaining = -1;
        official.hulks_message_budget = 0;
        import(&mut app, official, Some(previous));
        let session = app.session.as_ref().unwrap();
        assert_eq!(session.raw_score_for(MatchType::GoldenGoal), (2, 0));
        assert_eq!(session.raw_score_for(MatchType::Official), (3, 0));
        assert!(session.is_game_finished(MatchType::GoldenGoal));
        assert!(session.is_game_finished(MatchType::Official));
        assert_eq!(session.entries[0].gc_seconds_remaining, Some(42));
        assert_eq!(session.entries[1].gc_seconds_remaining, Some(-3));
        assert_eq!(session.entries[2].gc_seconds_remaining, Some(600));
        assert_eq!(session.entries[3].gc_seconds_remaining, Some(540));
        assert_eq!(session.entries[4].kind, EventKind::GoalHome);
        let markdown = session.markdown();
        assert!(markdown.contains("final score: HULKs 2:0 Invisibles"));
        assert!(markdown.contains("Final score: HULKs 3:0 HTWK"));
        for budget in [17, 700, 0] {
            assert!(markdown.contains(&format!("HULKs remaining message budget: {budget}")));
        }
    }

    #[test]
    fn combined_reopening_preserves_opponent_marker_and_rejects_old_test_packets() {
        let mut app = fixed_app(MatchType::Combined);
        let mut ended = snapshot(2);
        ended.state = 4;
        import(&mut app, ended, None);
        let saved = serde_json::to_string(app.session.as_ref().unwrap()).unwrap();
        app.session = Some(serde_json::from_str(&saved).unwrap());
        let mut official = snapshot(0);
        official.opponent_number = 9;
        official.state = 1;
        import(&mut app, official, None);
        let saved = serde_json::to_string(app.session.as_ref().unwrap()).unwrap();
        app.session = Some(serde_json::from_str(&saved).unwrap());
        let frozen = app.session.as_ref().unwrap().clone();
        import(&mut app, snapshot(8), None);
        assert_eq!(*app.session.as_ref().unwrap(), frozen);
        app.controller = Some(Receiver::for_test());
        app.poll_controller();
        let receiver = app.controller.as_ref().unwrap();
        assert_eq!(receiver.ignored_opponent, Some(4));
        assert!(!receiver.allow_opponent_change);
        assert_eq!(frozen.metadata.away, "HTWK");
        assert_eq!(frozen.golden_goal_opponent, Some(4));
    }

    #[test]
    fn combined_pending_transition_keeps_tab_capture_and_does_not_reopen_receiver_gate() {
        let mut app = fixed_app(MatchType::Combined);
        let mut ended = snapshot(1);
        ended.state = 4;
        import(&mut app, ended, None);
        app.controller = Some(Receiver::for_test());
        app.poll_controller();
        assert!(app.controller.as_ref().unwrap().allow_opponent_change);
        press(&mut app, KeyCode::Tab);
        let mut ready = snapshot(0);
        ready.opponent_number = 9;
        ready.state = 1;
        let later = time() + chrono::Duration::seconds(5);
        app.pending_controller.push(Update {
            capture_id: None,
            snapshot: ready,
            previous: None,
            received_at: later,
        });
        app.poll_controller();
        assert!(matches!(app.mode, Mode::Actions { timestamp, .. } if timestamp == time()));
        assert_eq!(app.pending_controller.len(), 1);
        assert!(!app.controller.as_ref().unwrap().allow_opponent_change);
        press(&mut app, KeyCode::Esc);
        app.poll_controller();
        let session = app.session.as_ref().unwrap();
        assert_eq!(session.entries.last().unwrap().timestamp, later);
        assert_eq!(session.current_game(), MatchType::Official);
        assert_eq!(session.entries.last().unwrap().kind, EventKind::Custom);
    }

    #[test]
    fn combined_transition_preserves_draft_caret_timestamp_and_open_correction() {
        let mut app = fixed_app(MatchType::Combined);
        press(&mut app, KeyCode::Enter);
        app.paste("ac");
        press(&mut app, KeyCode::Left);
        let mut ended = snapshot(1);
        ended.state = 4;
        import(&mut app, ended, None);
        let mut official = snapshot(0);
        official.opponent_number = 9;
        official.state = 1;
        import(&mut app, official.clone(), None);
        assert!(matches!(app.mode, Mode::Writing));
        press(&mut app, KeyCode::Char('b'));
        assert_eq!(app.note_editor.lines()[0], "abc");
        assert_eq!(app.session.as_ref().unwrap().entries[0].timestamp, time());
        assert_eq!(
            app.session.as_ref().unwrap().entries[0].game,
            Some(MatchType::GoldenGoal)
        );
        press(&mut app, KeyCode::Enter);
        app.history.select(Some(0));
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Char('e'));
        app.paste(" corrected");
        let mut playing = official.clone();
        playing.state = 3;
        import(&mut app, playing, Some(official));
        assert!(matches!(app.mode, Mode::Form(_)));
        assert_eq!(app.history.selected(), Some(0));
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Enter);
        assert_eq!(
            app.session.as_ref().unwrap().entries[0].text,
            "abc corrected"
        );
        assert_eq!(
            app.session.as_ref().unwrap().entries[0].game,
            Some(MatchType::GoldenGoal)
        );
    }

    #[test]
    fn fixed_golden_goal_imports_baseline_then_goals_with_controller_clocks() {
        let mut app = fixed_app(MatchType::GoldenGoal);
        let mut baseline = snapshot(3);
        baseline.seconds_remaining = 42;
        import(&mut app, baseline.clone(), None);
        let mut goal = snapshot(4);
        goal.seconds_remaining = -2;
        import(&mut app, goal, Some(baseline));
        let session = app.session.as_mut().unwrap();
        assert_eq!(session.entries[0].kind, EventKind::GoldenGoalStart);
        assert_eq!(session.entries[0].score_delta, Some([3, 0]));
        assert_eq!(session.entries[1].kind, EventKind::GoalHome);
        assert_eq!(session.entries[1].score_delta, None);
        assert_eq!(session.entries[0].gc_seconds_remaining, Some(42));
        assert_eq!(session.entries[1].gc_seconds_remaining, Some(-2));
        assert!(
            session
                .entries
                .iter()
                .all(|entry| entry.timestamp == time())
        );
        assert_eq!(session.score(), (4, 0));
        assert_eq!(session.golden_goal_score(), (4, 0));
        assert!(session.in_golden_goal());
        assert_eq!(session.current_teams(), ("HULKs", "Invisibles"));
        let saved = serde_json::to_string(session).unwrap();
        assert_eq!(serde_json::from_str::<Session>(&saved).unwrap(), *session);
        session.delete(1);
        assert_eq!(session.score(), (3, 0));
        session.restore(1);
        assert_eq!(session.score(), (4, 0));
    }

    #[test]
    fn fixed_golden_goal_corrections_and_reconnects_are_signed_and_reversible() {
        let mut app = fixed_app(MatchType::GoldenGoal);
        import(&mut app, snapshot(3), None);
        import(&mut app, snapshot(2), Some(snapshot(3)));
        import(&mut app, snapshot(5), Some(snapshot(2)));
        import(&mut app, snapshot(7), None);
        let session = app.session.as_mut().unwrap();
        assert_eq!(session.entries[1].score_delta, Some([-1, 0]));
        assert_eq!(session.entries[2].score_delta, Some([3, 0]));
        assert_eq!(session.entries[3].score_delta, Some([2, 0]));
        assert_eq!(session.entries[3].kind, EventKind::Custom);
        assert_eq!(session.score(), (7, 0));
        session.delete(3);
        assert_eq!(session.score(), (5, 0));
        session.restore(3);
        assert_eq!(session.score(), (7, 0));
        session.delete(0);
        session.delete(2);
        session.delete(3);
        assert_eq!(session.raw_score(), (-1, 0));
        import(&mut app, snapshot(2), None);
        assert_eq!(app.session.as_ref().unwrap().score(), (2, 0));
    }

    #[test]
    fn fixed_golden_goal_maps_both_teams_when_hulks_are_away() {
        let mut app = fixed_app(MatchType::GoldenGoal);
        let metadata = &mut app.session.as_mut().unwrap().metadata;
        std::mem::swap(&mut metadata.home, &mut metadata.away);
        import(&mut app, snapshot(0), None);
        import(&mut app, snapshot(1), Some(snapshot(0)));
        let mut opponent_goal = snapshot(1);
        opponent_goal.opponent_score = 1;
        import(&mut app, opponent_goal, Some(snapshot(1)));
        let session = app.session.as_ref().unwrap();
        assert_eq!(session.entries[1].kind, EventKind::GoalAway);
        assert_eq!(session.entries[2].kind, EventKind::GoalHome);
        assert_eq!(session.current_teams(), ("Invisibles", "HULKs"));
        assert_eq!(session.score(), (1, 1));
    }

    #[test]
    fn fixed_golden_goal_never_enters_official_halves_or_repeats_its_start() {
        let mut app = fixed_app(MatchType::GoldenGoal);
        app.session
            .as_mut()
            .unwrap()
            .record(EventKind::GoldenGoalStart, time());
        import(&mut app, snapshot(0), None);
        let mut second_half = snapshot(1);
        second_half.first_half = false;
        import(&mut app, second_half.clone(), Some(snapshot(0)));
        let mut shootout = second_half.clone();
        shootout.game_phase = 1;
        shootout.hulks_score = 7;
        import(&mut app, shootout.clone(), Some(second_half));
        let mut overtime = snapshot(2);
        overtime.game_phase = 2;
        import(&mut app, overtime, Some(shootout));
        let session = app.session.as_ref().unwrap();
        assert_eq!(session.match_type, Some(MatchType::GoldenGoal));
        assert!(session.in_golden_goal());
        assert_eq!(session.score(), (2, 0));
        assert_eq!(session.entries[2].kind, EventKind::GoalHome);
        assert_eq!(session.entries[3].score_delta, None);
        assert_eq!(
            session
                .entries
                .iter()
                .map(|entry| entry.kind)
                .collect::<Vec<_>>(),
            [
                EventKind::GoldenGoalStart,
                EventKind::Custom,
                EventKind::GoalHome,
                EventKind::Custom,
                EventKind::Custom,
            ]
        );
    }

    #[test]
    fn fixed_golden_goal_finishes_either_half_using_ending_packet_budget() {
        for first_half in [true, false] {
            let mut app = fixed_app(MatchType::GoldenGoal);
            let start = snapshot(0);
            import(&mut app, start.clone(), None);
            let mut finished = snapshot(1);
            finished.first_half = first_half;
            finished.state = 4;
            finished.hulks_message_budget = 17;
            finished.seconds_remaining = -3;
            import(&mut app, finished, Some(start));
            let session = app.session.as_ref().unwrap();
            let entry = &session.entries[1];
            assert_eq!(entry.kind, EventKind::GameEnd);
            assert_eq!(entry.score_delta, Some([1, 0]));
            assert_eq!(entry.gc_seconds_remaining, Some(-3));
            assert_eq!(entry.message_budget, Some(17));
            assert_eq!(entry.golden_goal_message_budget, None);
            assert!(session.is_finished());
            assert!(session.in_golden_goal());
            assert_eq!(session.score(), (1, 0));
            let markdown = session.markdown();
            assert!(markdown.contains("HULKs remaining message budget: 17"));
            assert!(!markdown.contains("HULKs remaining message budget: 1200"));
        }
    }

    #[test]
    fn fixed_golden_goal_can_attach_finished_without_inventing_a_start() {
        let mut app = fixed_app(MatchType::GoldenGoal);
        let mut finished = snapshot(1);
        finished.state = 4;
        import(&mut app, finished, None);
        let session = app.session.as_ref().unwrap();
        assert_eq!(session.entries.len(), 1);
        assert_eq!(session.entries[0].kind, EventKind::GameEnd);
        assert_eq!(session.entries[0].score_delta, Some([1, 0]));
        assert_eq!(session.score(), (1, 0));
        assert!(session.is_finished());
    }

    #[test]
    fn legacy_imports_continue_after_game_end() {
        let mut app = app();
        app.session
            .as_mut()
            .unwrap()
            .record(EventKind::GameEnd, time());
        import(&mut app, snapshot(0), None);
        import(&mut app, snapshot(1), Some(snapshot(0)));
        let session = app.session.as_ref().unwrap();
        assert_eq!(session.entries.len(), 3);
        assert_eq!(session.entries[1].kind, EventKind::GameStart);
        assert_eq!(session.entries[2].kind, EventKind::GoalHome);
        assert_eq!(session.score(), (1, 0));
    }

    #[test]
    fn fixed_games_ignore_imports_after_end_until_the_end_is_removed() {
        for match_type in [MatchType::GoldenGoal, MatchType::Official] {
            let mut app = fixed_app(match_type);
            import(&mut app, snapshot(0), None);
            let mut finished = snapshot(0);
            finished.first_half = false;
            finished.state = 4;
            import(&mut app, finished.clone(), Some(snapshot(0)));
            let frozen = app.session.as_ref().unwrap().clone();
            let mut live = snapshot(1);
            live.seconds_remaining = 123;
            let mut receiver = Receiver::for_test();
            receiver.latest = Some(live.clone());
            app.controller = Some(receiver);
            app.pending_controller.push(Update {
                capture_id: None,
                snapshot: live.clone(),
                previous: Some(finished),
                received_at: time(),
            });
            app.dirty = false;
            app.poll_controller();
            import(&mut app, snapshot(3), None);
            assert_eq!(*app.session.as_ref().unwrap(), frozen);
            assert!(!app.dirty);
            assert!(app.pending_controller.is_empty());
            assert_eq!(app.controller.as_ref().unwrap().latest, Some(live.clone()));
            app.history.select(Some(1));
            press(&mut app, KeyCode::Tab);
            press(&mut app, KeyCode::Char('d'));
            press(&mut app, KeyCode::Char('y'));
            assert!(app.controller.as_ref().unwrap().latest.is_none());
            assert!(app.pending_controller.is_empty());
            import(&mut app, snapshot(2), None);
            let session = app.session.as_mut().unwrap();
            assert!(!session.is_finished());
            assert_eq!(session.entries[2].kind, EventKind::Custom);
            assert_eq!(session.entries[2].score_delta, Some([2, 0]));
            assert!(session.entries[2].text.starts_with("GC sync"));
            assert_eq!(session.score(), (2, 0));
            session.restore(1);
            assert!(session.is_finished());
            import(&mut app, snapshot(3), Some(snapshot(2)));
            let session = app.session.as_ref().unwrap();
            assert_eq!(session.entries.len(), 3);
            assert!(
                session
                    .markdown()
                    .contains("final score: HULKs 2:0 Invisibles")
                    || session.markdown().contains("Final score: HULKs 2:0 HTWK")
            );
        }
    }

    #[test]
    fn correcting_an_end_marker_rebaselines_and_discards_closed_period_deltas() {
        for match_type in [MatchType::GoldenGoal, MatchType::Official] {
            let mut app = fixed_app(match_type);
            import(&mut app, snapshot(0), None);
            app.session
                .as_mut()
                .unwrap()
                .record(EventKind::GameEnd, time());
            let mut receiver = Receiver::for_test();
            receiver.latest = Some(snapshot(1));
            app.controller = Some(receiver);
            app.history.select(Some(1));
            press(&mut app, KeyCode::Tab);
            press(&mut app, KeyCode::Char('e'));
            if let Mode::Form(form) = &mut app.mode {
                form.fields[0].editor =
                    tui_textarea::TextArea::new(vec![EventKind::Custom.label().into()]);
                form.selected = form.fields.len();
            }
            app.pending_controller.push(Update {
                capture_id: None,
                snapshot: snapshot(2),
                previous: Some(snapshot(1)),
                received_at: time(),
            });
            press(&mut app, KeyCode::Enter);
            assert!(!app.session.as_ref().unwrap().is_finished());
            assert!(app.controller.as_ref().unwrap().latest.is_none());
            assert!(app.pending_controller.is_empty());
            import(&mut app, snapshot(2), None);
            assert_eq!(app.session.as_ref().unwrap().score(), (2, 0));
            assert_eq!(
                app.session
                    .as_ref()
                    .unwrap()
                    .entries
                    .last()
                    .unwrap()
                    .score_delta,
                Some([2, 0])
            );
        }
    }

    #[test]
    fn fixed_official_imports_keep_half_rules_and_all_controller_clocks() {
        for game_phase in [0, 2] {
            let mut app = fixed_app(MatchType::Official);
            let mut current = snapshot(0);
            current.game_phase = game_phase;
            import(&mut app, current.clone(), None);
            let mut previous = current.clone();
            current.hulks_score = 1;
            current.seconds_remaining = 500;
            import(&mut app, current.clone(), Some(previous));
            previous = current.clone();
            current.state = 4;
            current.seconds_remaining = -1;
            import(&mut app, current.clone(), Some(previous));
            previous = current.clone();
            current.state = 3;
            current.first_half = false;
            current.seconds_remaining = 600;
            import(&mut app, current.clone(), Some(previous));
            previous = current.clone();
            current.state = 4;
            current.seconds_remaining = 0;
            import(&mut app, current, Some(previous));
            let session = app.session.as_ref().unwrap();
            assert_eq!(session.score(), (1, 0));
            assert_eq!(
                session
                    .entries
                    .iter()
                    .map(|entry| (entry.kind, entry.gc_seconds_remaining))
                    .collect::<Vec<_>>(),
                [
                    (EventKind::GameStart, Some(540)),
                    (EventKind::GoalHome, Some(500)),
                    (EventKind::HalfTime, Some(-1)),
                    (EventKind::SecondHalfStart, Some(600)),
                    (EventKind::GameEnd, Some(0)),
                ]
            );
            assert!(
                session
                    .entries
                    .iter()
                    .all(|entry| entry.timestamp == time())
            );
        }
    }

    #[test]
    fn official_statistics_baselines_do_not_invent_penalties_or_restarts() {
        for (state, first_half, score, pre_game) in [
            (0, true, 0, true),
            (1, true, 0, false),
            (2, true, 0, false),
            (3, true, 0, false),
            (1, false, 0, false),
            (1, true, 1, false),
        ] {
            let mut app = fixed_app(MatchType::Official);
            let mut current = snapshot(score);
            current.state = state;
            current.first_half = first_half;
            current.hulks_penalties[19] = 2;
            current.set_play = 4;
            current.kicking_team = current.opponent_number;
            import(&mut app, current.clone(), None);
            let statistics = app.session.as_ref().unwrap().entries[0]
                .statistics
                .as_ref()
                .unwrap();
            let baseline = statistics.baseline.as_ref().unwrap();
            assert_eq!(baseline.penalties, current.hulks_penalties);
            assert_eq!(baseline.pre_game, pre_game);
            assert!(statistics.penalties.is_empty());
            assert!(!statistics.opponent_restart);
        }
        let mut app = fixed_app(MatchType::Official);
        let mut current = snapshot(0);
        current.state = 1;
        current.opponent_score = 1;
        import(&mut app, current, None);
        assert!(
            !app.session.as_ref().unwrap().entries[0]
                .statistics
                .as_ref()
                .unwrap()
                .baseline
                .as_ref()
                .unwrap()
                .pre_game
        );
    }

    #[test]
    fn only_initial_attachment_can_establish_complete_statistics_coverage() {
        for state in [0, 1, 2] {
            let mut app = fixed_app(MatchType::Official);
            let mut initial = snapshot(0);
            initial.state = state;
            import(&mut app, initial.clone(), None);
            let mut ended = initial.clone();
            ended.state = 4;
            ended.first_half = false;
            import(&mut app, ended, Some(initial));
            let markdown = app.session.as_ref().unwrap().markdown();
            assert_eq!(markdown.contains("partial GC coverage"), state != 0);
            assert_eq!(markdown.contains("Goal share: unavailable"), state != 0);
        }
    }

    #[test]
    fn official_statistics_rebaseline_after_gaps_phases_and_opponent_changes() {
        let mut app = fixed_app(MatchType::Official);
        let mut current = snapshot(0);
        current.hulks_penalties[0] = 2;
        current.set_play = 5;
        current.kicking_team = current.opponent_number;
        import(&mut app, snapshot(0), None);
        for previous in [
            None,
            Some({
                let mut previous = snapshot(0);
                previous.game_phase = 1;
                previous
            }),
            Some({
                let mut previous = snapshot(0);
                previous.game_phase = 255;
                previous
            }),
            Some({
                let mut previous = snapshot(0);
                previous.opponent_number = 7;
                previous
            }),
        ] {
            import(&mut app, current.clone(), previous);
            let statistics = app
                .session
                .as_ref()
                .unwrap()
                .entries
                .last()
                .unwrap()
                .statistics
                .as_ref()
                .unwrap();
            assert_eq!(
                statistics.baseline.as_ref().unwrap().penalties,
                current.hulks_penalties
            );
            assert!(!statistics.baseline.as_ref().unwrap().pre_game);
            assert!(statistics.penalties.is_empty());
            assert!(!statistics.opponent_restart);
        }
        let mut overtime = current.clone();
        overtime.game_phase = 2;
        overtime.hulks_penalties[0] = 5;
        import(&mut app, overtime, Some(current));
        let statistics = app
            .session
            .as_ref()
            .unwrap()
            .entries
            .last()
            .unwrap()
            .statistics
            .as_ref()
            .unwrap();
        assert!(statistics.baseline.is_none());
        assert_eq!(statistics.penalties[0].before, 2);
        assert_eq!(statistics.penalties[0].after, 5);
        assert!(!statistics.opponent_restart);
    }

    #[test]
    fn official_penalty_reasons_releases_and_ending_packet_are_retained() {
        for game_phase in [0, 2] {
            let mut app = fixed_app(MatchType::Official);
            let mut current = snapshot(0);
            current.game_phase = game_phase;
            import(&mut app, current.clone(), None);
            for code in [2, 5, 0, 11, 13, 0, 2] {
                let previous = current.clone();
                current.hulks_penalties[19] = code;
                import(&mut app, current.clone(), Some(previous.clone()));
                let session = app.session.as_mut().unwrap();
                let index = session.entries.len() - 1;
                let entry = session.entries[index].clone();
                assert_eq!(entry.kind, EventKind::Custom);
                let statistics = entry.statistics.as_ref().unwrap();
                assert!(statistics.baseline.is_none());
                assert_eq!(statistics.penalties.len(), 1);
                let change = &statistics.penalties[0];
                assert_eq!(
                    (change.player, change.before, change.after),
                    (20, previous.hulks_penalties[19], code)
                );
                assert!(
                    entry
                        .text
                        .contains(&format!("player 20 {}→{}", change.before, code))
                );
                assert!(entry.text.ends_with(&current.summary()));
                assert!(!session.markdown().contains("HULKs penalties:"));
                session.delete(index);
                session.restore(index);
                assert_eq!(session.entries[index], entry);
            }
            let previous = current.clone();
            current.first_half = false;
            current.state = 4;
            current.hulks_penalties[19] = 0;
            import(&mut app, current.clone(), Some(previous));
            let session = app.session.as_ref().unwrap();
            let ending = session.entries.last().unwrap();
            assert_eq!(ending.kind, EventKind::GameEnd);
            assert_eq!(ending.statistics.as_ref().unwrap().penalties[0].after, 0);
            let frozen = session.clone();
            let previous = current.clone();
            current.hulks_penalties[19] = 2;
            import(&mut app, current, Some(previous));
            assert_eq!(app.session.as_ref().unwrap(), &frozen);
        }
    }

    #[test]
    fn opponent_restart_statistics_count_new_awards_not_clock_or_state_changes() {
        let mut app = fixed_app(MatchType::Official);
        let mut current = snapshot(0);
        current.set_play = 4;
        current.kicking_team = current.opponent_number;
        import(&mut app, current.clone(), None);
        for (set_play, kicking_team, state, expected) in [
            (4, 4, 2, false),
            (4, 4, 3, false),
            (4, 4, 3, false),
            (4, 24, 3, false),
            (4, 4, 3, true),
            (5, 4, 3, true),
            (5, 4, 2, false),
            (6, 4, 3, false),
            (1, 4, 3, false),
            (0, 4, 3, false),
            (5, 4, 3, true),
            (5, 24, 3, false),
        ] {
            let previous = current.clone();
            current.set_play = set_play;
            current.kicking_team = kicking_team;
            current.state = state;
            current.seconds_remaining -= 1;
            import(&mut app, current.clone(), Some(previous));
            let statistics = app
                .session
                .as_ref()
                .unwrap()
                .entries
                .last()
                .unwrap()
                .statistics
                .as_ref()
                .unwrap();
            assert!(statistics.baseline.is_none());
            assert!(statistics.penalties.is_empty());
            assert_eq!(statistics.opponent_restart, expected);
        }
    }

    #[test]
    fn golden_goal_penalty_changes_never_add_json_records_or_statistics() {
        for match_type in [Some(MatchType::GoldenGoal), Some(MatchType::Combined), None] {
            let mut app = match match_type {
                Some(match_type) => fixed_app(match_type),
                None => {
                    let mut app = app();
                    app.session
                        .as_mut()
                        .unwrap()
                        .record(EventKind::GoldenGoalStart, time());
                    app
                }
            };
            let mut current = snapshot(0);
            current.state = 1;
            import(&mut app, current.clone(), None);
            let saved = serde_json::to_string(app.session.as_ref().unwrap()).unwrap();
            app.dirty = false;
            let previous = current.clone();
            current.hulks_penalties[0] = 2;
            import(&mut app, current.clone(), Some(previous));
            assert_eq!(
                serde_json::to_string(app.session.as_ref().unwrap()).unwrap(),
                saved
            );
            assert!(!app.dirty);
            let previous = current.clone();
            current.set_play = 4;
            current.kicking_team = current.opponent_number;
            import(&mut app, current, Some(previous));
            assert!(
                app.session
                    .as_ref()
                    .unwrap()
                    .entries
                    .iter()
                    .all(|entry| entry.statistics.is_none())
            );
        }
    }

    #[test]
    fn legacy_official_markers_baseline_without_counting_test_events() {
        for state in [3, 4] {
            let mut app = app();
            app.session
                .as_mut()
                .unwrap()
                .record(EventKind::GoldenGoalStart, time());
            let mut previous = snapshot(0);
            previous.state = 1;
            import(&mut app, previous.clone(), None);
            assert!(
                app.session
                    .as_ref()
                    .unwrap()
                    .entries
                    .last()
                    .unwrap()
                    .statistics
                    .is_none()
            );
            let mut current = previous.clone();
            current.state = state;
            current.hulks_penalties[0] = 2;
            current.set_play = 4;
            current.kicking_team = current.opponent_number;
            import(&mut app, current.clone(), Some(previous));
            let session = app.session.as_ref().unwrap();
            assert_eq!(session.current_game(), MatchType::Official);
            let statistics = session.entries.last().unwrap().statistics.as_ref().unwrap();
            assert_eq!(
                statistics.baseline.as_ref().unwrap().penalties,
                current.hulks_penalties
            );
            assert!(statistics.penalties.is_empty());
            assert!(!statistics.opponent_restart);
        }
    }

    #[test]
    fn shootout_and_unknown_phases_have_no_statistics() {
        for game_phase in [1, 255] {
            let mut app = fixed_app(MatchType::Official);
            let previous = snapshot(0);
            import(&mut app, previous.clone(), None);
            let mut current = previous.clone();
            current.game_phase = game_phase;
            current.hulks_penalties[0] = 2;
            current.set_play = 4;
            current.kicking_team = current.opponent_number;
            import(&mut app, current, Some(previous));
            assert!(
                app.session
                    .as_ref()
                    .unwrap()
                    .entries
                    .last()
                    .unwrap()
                    .statistics
                    .is_none()
            );
        }
    }

    #[test]
    fn timeout_preserves_statistics_coverage_and_penalties_without_counting_restarts() {
        let mut app = fixed_app(MatchType::Official);
        let mut initial = snapshot(0);
        initial.state = 0;
        import(&mut app, initial.clone(), None);
        let playing = snapshot(0);
        import(&mut app, playing.clone(), Some(initial));
        let mut timeout = playing.clone();
        timeout.game_phase = 3;
        timeout.set_play = 5;
        timeout.kicking_team = timeout.opponent_number;
        timeout.hulks_penalties[0] = 5;
        import(&mut app, timeout.clone(), Some(playing));
        let stats = app
            .session
            .as_ref()
            .unwrap()
            .entries
            .last()
            .unwrap()
            .statistics
            .as_ref()
            .unwrap();
        assert!(stats.baseline.is_none());
        assert_eq!(
            stats.penalties,
            vec![PenaltyChange {
                player: 1,
                before: 0,
                after: 5
            }]
        );
        assert!(!stats.opponent_restart);
        let mut resumed = timeout.clone();
        resumed.game_phase = 0;
        import(&mut app, resumed.clone(), Some(timeout));
        let stats = app
            .session
            .as_ref()
            .unwrap()
            .entries
            .last()
            .unwrap()
            .statistics
            .as_ref()
            .unwrap();
        assert!(stats.baseline.is_none());
        assert!(stats.penalties.is_empty());
        assert!(!stats.opponent_restart);
        let mut end = resumed.clone();
        end.state = 4;
        end.first_half = false;
        end.seconds_remaining = 0;
        import(&mut app, end, Some(resumed));
        let markdown = app.session.as_ref().unwrap().markdown();
        assert!(markdown.contains("#1 1/1"));
        assert!(markdown.contains("0 observed opponent goal-kicks/kick-ins"));
        assert!(!markdown.contains("partial GC coverage"));

        let mut app = fixed_app(MatchType::Official);
        let mut overtime = snapshot(0);
        overtime.game_phase = 2;
        overtime.state = 1;
        import(&mut app, overtime, None);
        assert!(
            !app.session.as_ref().unwrap().entries[0]
                .statistics
                .as_ref()
                .unwrap()
                .baseline
                .as_ref()
                .unwrap()
                .pre_game
        );
    }

    #[test]
    fn attaching_mid_game_syncs_without_inventing_historical_goals() {
        let mut app = app();
        import(&mut app, snapshot(3), None);
        let session = app.session.as_ref().unwrap();
        assert_eq!(session.score(), (3, 0));
        assert_eq!(session.entries.len(), 1);
        assert_eq!(session.entries[0].kind, EventKind::GameStart);
        assert_eq!(session.entries[0].score_delta, Some([3, 0]));
        assert!(session.entries[0].text.contains("sync"));
        import(&mut app, snapshot(4), Some(snapshot(3)));
        let session = app.session.as_mut().unwrap();
        assert_eq!(session.entries[1].kind, EventKind::GoalHome);
        assert_eq!(session.score(), (4, 0));
        session.delete(1);
        assert_eq!(session.score(), (3, 0));
        session.restore(1);
        assert_eq!(session.score(), (4, 0));
    }

    #[test]
    fn score_decreases_jumps_and_reconnects_are_reversible_adjustments() {
        let mut app = app();
        import(&mut app, snapshot(3), None);
        import(&mut app, snapshot(2), Some(snapshot(3)));
        assert_eq!(
            app.session.as_ref().unwrap().entries[1].score_delta,
            Some([-1, 0])
        );
        import(&mut app, snapshot(5), Some(snapshot(2)));
        assert_eq!(
            app.session.as_ref().unwrap().entries[2].score_delta,
            Some([3, 0])
        );
        import(&mut app, snapshot(7), None);
        let session = app.session.as_mut().unwrap();
        assert_eq!(session.score(), (7, 0));
        assert_eq!(session.entries.len(), 4);
        assert_eq!(session.entries[3].score_delta, Some([2, 0]));
        assert!(
            session
                .entries
                .iter()
                .all(|entry| entry.kind != EventKind::GoalHome)
        );
        session.delete(3);
        assert_eq!(session.score(), (5, 0));
        session.restore(3);
        assert_eq!(session.score(), (7, 0));
    }

    #[test]
    fn imports_preserve_draft_caret_and_an_open_correction() {
        let mut app = app();
        press(&mut app, KeyCode::Enter);
        app.paste("ac");
        press(&mut app, KeyCode::Left);
        import(&mut app, snapshot(0), None);
        press(&mut app, KeyCode::Char('b'));
        assert_eq!(app.note_editor.lines()[0], "abc");
        assert_eq!(app.session.as_ref().unwrap().entries[0].timestamp, time());
        press(&mut app, KeyCode::Enter);
        app.history.select(Some(0));
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Char('e'));
        app.paste(" corrected");
        import(&mut app, snapshot(1), Some(snapshot(0)));
        assert!(matches!(app.mode, Mode::Form(_)));
        assert_eq!(app.history.selected(), Some(0));
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Enter);
        assert_eq!(
            app.session.as_ref().unwrap().entries[0].text,
            "abc corrected"
        );
    }

    #[test]
    fn queued_penalty_events_preserve_note_caret_timestamps_and_correction_payload() {
        let mut app = fixed_app(MatchType::Official);
        let previous = snapshot(0);
        import(&mut app, previous.clone(), None);
        press(&mut app, KeyCode::Enter);
        app.paste("ac");
        press(&mut app, KeyCode::Left);
        press(&mut app, KeyCode::Tab);
        let mut current = previous.clone();
        current.hulks_penalties[0] = 2;
        let received_at = time() + chrono::Duration::seconds(3);
        app.pending_controller.push(Update {
            capture_id: None,
            snapshot: current,
            previous: Some(previous),
            received_at,
        });
        app.poll_controller();
        assert_eq!(app.pending_controller.len(), 1);
        assert_eq!(app.session.as_ref().unwrap().entries.len(), 2);
        press(&mut app, KeyCode::Esc);
        app.poll_controller();
        press(&mut app, KeyCode::Char('b'));
        assert_eq!(app.note_editor.lines()[0], "abc");
        let session = app.session.as_ref().unwrap();
        assert_eq!(session.active_note, Some(1));
        assert_eq!(session.entries[1].timestamp, time());
        assert_eq!(session.entries[2].timestamp, received_at);
        let statistics = session.entries[2].statistics.clone();
        assert!(statistics.is_some());
        press(&mut app, KeyCode::Enter);
        app.history.select(Some(2));
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Char('e'));
        app.paste(" corrected");
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Enter);
        let entry = &app.session.as_ref().unwrap().entries[2];
        assert!(entry.text.ends_with(" corrected"));
        assert_eq!(entry.statistics, statistics);
        assert_eq!(entry.timestamp, received_at);
    }

    #[test]
    fn official_goal_keys_are_locked_but_test_goals_and_notes_work() {
        let mut app = app();
        app.controller = Some(Receiver::for_test());
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Char('1'));
        assert!(app.session.as_ref().unwrap().entries.is_empty());
        press(&mut app, KeyCode::F(6));
        assert!(app.session.as_ref().unwrap().entries.is_empty());
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Char('t'));
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Char('1'));
        assert_eq!(app.session.as_ref().unwrap().golden_goal_score(), (1, 0));
        press(&mut app, KeyCode::Enter);
        app.paste("deployment");
        import(&mut app, snapshot(0), None);
        assert!(matches!(app.mode, Mode::Writing));
        assert!(!app.session.as_ref().unwrap().in_golden_goal());
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Char('2'));
        assert_eq!(app.session.as_ref().unwrap().score(), (0, 0));
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Char('l'));
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Char('2'));
        assert_eq!(app.session.as_ref().unwrap().score(), (0, 1));
    }

    #[test]
    fn away_side_mapping_and_shootout_do_not_corrupt_official_score() {
        let mut app = app();
        let metadata = &mut app.session.as_mut().unwrap().metadata;
        std::mem::swap(&mut metadata.home, &mut metadata.away);
        import(&mut app, snapshot(0), None);
        import(&mut app, snapshot(1), Some(snapshot(0)));
        assert_eq!(app.session.as_ref().unwrap().score(), (0, 1));
        assert_eq!(
            app.session.as_ref().unwrap().entries[1].kind,
            EventKind::GoalAway
        );
        let mut shootout = snapshot(7);
        shootout.game_phase = 1;
        import(&mut app, shootout, Some(snapshot(1)));
        assert_eq!(app.session.as_ref().unwrap().score(), (0, 1));
    }

    #[test]
    fn captured_test_goal_precedes_controller_events_received_while_menu_is_open() {
        let mut app = app();
        app.controller = Some(Receiver::for_test());
        app.session
            .as_mut()
            .unwrap()
            .record(EventKind::GoldenGoalStart, time());
        press(&mut app, KeyCode::Tab);
        app.pending_controller.push(Update {
            capture_id: None,
            snapshot: snapshot(0),
            previous: None,
            received_at: time(),
        });
        app.poll_controller();
        assert_eq!(app.pending_controller.len(), 1);
        press(&mut app, KeyCode::Char('1'));
        app.poll_controller();
        let session = app.session.as_ref().unwrap();
        assert_eq!(session.golden_goal_score(), (1, 0));
        assert_eq!(session.score(), (0, 0));
        assert_eq!(session.entries[1].kind, EventKind::GoalHome);
        assert_eq!(session.entries[2].kind, EventKind::GameStart);
        assert!(app.pending_controller.is_empty());
    }

    #[test]
    fn reconnect_uses_unclamped_total_after_a_sync_is_removed() {
        let mut app = app();
        import(&mut app, snapshot(3), None);
        import(&mut app, snapshot(2), Some(snapshot(3)));
        app.session.as_mut().unwrap().delete(0);
        assert_eq!(app.session.as_ref().unwrap().raw_score(), (-1, 0));
        assert_eq!(app.session.as_ref().unwrap().score(), (0, 0));
        import(&mut app, snapshot(2), None);
        assert_eq!(app.session.as_ref().unwrap().score(), (2, 0));
    }

    #[test]
    fn a_finished_packet_leaves_test_mode_without_inventing_a_start() {
        for first_half in [true, false] {
            let mut app = app();
            app.session
                .as_mut()
                .unwrap()
                .record(EventKind::GoldenGoalStart, time());
            let mut finished = snapshot(1);
            finished.first_half = first_half;
            finished.state = 4;
            import(&mut app, finished, Some(snapshot(0)));
            let session = app.session.as_ref().unwrap();
            assert!(!session.in_golden_goal());
            assert_eq!(session.score(), (1, 0));
            assert_eq!(session.golden_goal_score(), (0, 0));
            assert_eq!(
                session.entries[1].kind,
                if first_half {
                    EventKind::HalfTime
                } else {
                    EventKind::GameEnd
                }
            );
        }
    }

    #[test]
    fn manual_boundary_budget_is_captured_on_tab_and_stale_data_stays_unavailable() {
        use std::time::{Duration, Instant};
        let mut app = app();
        let mut receiver = Receiver::for_test();
        receiver.latest = Some(snapshot(0));
        receiver.received_at = Some(Instant::now());
        app.controller = Some(receiver);
        press(&mut app, KeyCode::Tab);
        app.controller
            .as_mut()
            .unwrap()
            .latest
            .as_mut()
            .unwrap()
            .hulks_message_budget = 900;
        press(&mut app, KeyCode::Char('h'));
        assert_eq!(
            app.session.as_ref().unwrap().entries[0].message_budget,
            Some(1200)
        );
        app.controller.as_mut().unwrap().received_at =
            Some(Instant::now() - Duration::from_secs(6));
        press(&mut app, KeyCode::Tab);
        app.controller.as_mut().unwrap().received_at = Some(Instant::now());
        press(&mut app, KeyCode::Char('f'));
        let session = app.session.as_ref().unwrap();
        assert_eq!(session.entries[1].message_budget, None);
        assert!(
            session
                .markdown()
                .contains("HULKs remaining message budget: unavailable")
        );
    }

    #[test]
    fn golden_goal_uses_pre_reset_budget_and_each_half_uses_its_ending_packet() {
        let mut app = app();
        let session = app.session.as_mut().unwrap();
        std::mem::swap(&mut session.metadata.home, &mut session.metadata.away);
        session.record(EventKind::GoldenGoalStart, time());
        let mut before_start = snapshot(0);
        before_start.state = 1;
        before_start.hulks_message_budget = 900;
        let start = snapshot(0);
        import(&mut app, start.clone(), Some(before_start));
        let mut halftime = start.clone();
        halftime.state = 4;
        halftime.hulks_message_budget = 700;
        import(&mut app, halftime.clone(), Some(start));
        let mut second_half = snapshot(0);
        second_half.first_half = false;
        import(&mut app, second_half.clone(), Some(halftime));
        let mut fulltime = second_half.clone();
        fulltime.state = 4;
        fulltime.hulks_message_budget = 0;
        import(&mut app, fulltime, Some(second_half));
        let session = app.session.as_mut().unwrap();
        let markdown = session.markdown();
        let summaries: Vec<_> = markdown
            .lines()
            .filter(|line| line.starts_with("HULKs remaining message budget:"))
            .collect();
        assert_eq!(
            summaries,
            [
                "HULKs remaining message budget: 900",
                "HULKs remaining message budget: 700",
                "HULKs remaining message budget: 0"
            ]
        );
        assert!(
            markdown.find("message budget: 900").unwrap()
                < markdown.find("## 1. Halbzeit").unwrap()
        );
        let saved = serde_json::to_string(session).unwrap();
        assert_eq!(
            serde_json::from_str::<Session>(&saved).unwrap().markdown(),
            markdown
        );
        session.delete(2);
        assert!(!session.markdown().contains("message budget: 700"));
        session.restore(2);
        assert_eq!(session.markdown(), markdown);
    }

    #[test]
    fn attaching_after_the_test_does_not_invent_its_budget() {
        let mut app = app();
        app.session
            .as_mut()
            .unwrap()
            .record(EventKind::GoldenGoalStart, time());
        import(&mut app, snapshot(0), None);
        let markdown = app.session.as_ref().unwrap().markdown();
        assert!(markdown.contains("HULKs remaining message budget: unavailable"));
        assert!(!markdown.contains("message budget: 1200"));
    }

    #[test]
    fn manual_phase_markers_are_not_duplicated_by_controller() {
        let mut app = app();
        app.session
            .as_mut()
            .unwrap()
            .record(EventKind::GameStart, time());
        import(&mut app, snapshot(0), None);
        let mut ready = snapshot(0);
        ready.state = 1;
        import(&mut app, ready.clone(), Some(snapshot(0)));
        import(&mut app, snapshot(0), Some(ready));
        assert_eq!(
            app.session
                .as_ref()
                .unwrap()
                .entries
                .iter()
                .filter(|e| e.kind == EventKind::GameStart)
                .count(),
            1
        );
    }
}
