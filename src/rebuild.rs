use std::collections::HashMap;

use crate::{
    capture::Packet,
    game_controller::{Snapshot, decode_capture},
    model::{Entry, EventKind, MatchType, Session},
    statistics::{PenaltyChange, Statistics, StatisticsBaseline},
};

/// Backfill statistics only: recorded goals, phases, edits and removals remain authoritative.
pub fn rebuild(session: &mut Session, packets: &[Packet]) -> Result<usize, String> {
    let snapshots = validate(session, packets)?;
    let official = official_entries(session);
    let linked: HashMap<_, _> = session
        .entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| entry.capture_id.map(|id| (id, index)))
        .collect();
    let closed = closed_periods(session, &official, packets, &snapshots);
    let mut candidate = session.clone();
    let mut changed = 0;
    let mut previous: Option<(&Packet, &Snapshot)> = None;

    for (packet, snapshot) in packets.iter().zip(&snapshots) {
        let Some(snapshot) = snapshot.as_ref() else {
            continue;
        };
        let existing = linked.get(&packet.id).copied();
        if !packet.active
            || packet.game != Some(MatchType::Official)
            || !matches!(snapshot.game_phase, 0 | 2 | 3)
            || session.match_type == Some(MatchType::GoldenGoal)
            || !official_at(session, &official, existing, packet.id)
            || closed.iter().any(|&(end, reopen)| {
                packet.id > end && reopen.is_none_or(|reopen| packet.id < reopen)
            })
        {
            previous = None;
            continue;
        }

        let continuous = previous.filter(|(last, before)| {
            !packet.baseline
                && last.source == packet.source
                && last.true_data == packet.true_data
                && snapshot.packet_number.wrapping_sub(before.packet_number) == 1
                && snapshot.opponent_number == before.opponent_number
                && (chrono::Duration::zero()..=chrono::Duration::seconds(5))
                    .contains(&packet.received_at.signed_duration_since(last.received_at))
        });
        // A crash can archive a goal/phase packet before its history entry is saved.
        // Never reconstruct it or silently claim complete coverage of the user's score.
        let missing_history = existing.is_none()
            && previous.is_some_and(|(_, before)| history_changed(before, snapshot));
        let mut statistics = derive(snapshot, continuous.map(|(_, snapshot)| snapshot));
        if missing_history {
            statistics = baseline(snapshot, false);
        }
        previous = Some((packet, snapshot));

        if let Some(index) = existing {
            if session.entries[index].statistics.is_none() {
                // Insertions may have shifted the original entry's index.
                let entry = candidate
                    .entries
                    .iter_mut()
                    .find(|entry| entry.capture_id == Some(packet.id))
                    .unwrap();
                entry.statistics = Some(statistics);
                changed += 1;
            }
        } else if statistics.baseline.is_some()
            || !statistics.penalties.is_empty()
            || statistics.opponent_restart
        {
            let index = insertion_index(&candidate, packet.id);
            candidate.entries.insert(
                index,
                Entry {
                    game: (session.match_type == Some(MatchType::Combined))
                        .then_some(MatchType::Official),
                    timestamp: packet.received_at,
                    gc_seconds_remaining: Some(snapshot.seconds_remaining),
                    kind: EventKind::Custom,
                    text: format!("GC received: statistics backfill; {}", snapshot.summary()),
                    undone: false,
                    from_controller: true,
                    message_budget: Some(snapshot.hulks_message_budget),
                    golden_goal_message_budget: None,
                    score_delta: None,
                    statistics: Some(statistics),
                    capture_id: Some(packet.id),
                    capture_after: None,
                },
            );
            if let Some(active) = &mut candidate.active_note
                && *active >= index
            {
                *active += 1;
            }
            changed += 1;
        }
    }
    candidate.validate()?;
    *session = candidate;
    Ok(changed)
}

fn validate(session: &Session, packets: &[Packet]) -> Result<Vec<Option<Snapshot>>, String> {
    session.validate()?;
    let mut snapshots = Vec::with_capacity(packets.len());
    for (index, packet) in packets.iter().enumerate() {
        if packet.id != index as u64 + 1 {
            return Err(format!(
                "Capture packet {} has a duplicate, missing or unordered ID",
                packet.id
            ));
        }
        if packet.game == Some(MatchType::Combined) {
            return Err(format!(
                "Capture packet {} has an invalid game scope",
                packet.id
            ));
        }
        let snapshot = if packet.accepted {
            Some(
                decode_capture(&packet.packet, packet.true_data)
                    .map_err(|error| format!("Capture packet {}: {error}", packet.id))?
                    .ok_or_else(|| {
                        format!(
                            "Accepted capture packet {} is not a selected HULKs packet",
                            packet.id
                        )
                    })?,
            )
        } else {
            None
        };
        snapshots.push(snapshot);
    }
    let mut links = HashMap::new();
    for (index, entry) in session.entries.iter().enumerate() {
        if let Some(id) = entry.capture_id {
            if id == 0 || id > packets.len() as u64 {
                return Err(format!(
                    "Entry {index} links to missing capture packet {id}"
                ));
            }
            if !entry.from_controller || snapshots[(id - 1) as usize].is_none() {
                return Err(format!(
                    "Entry {index} does not link to an accepted controller packet"
                ));
            }
            if links.insert(id, index).is_some() {
                return Err(format!("More than one entry links to capture packet {id}"));
            }
        }
        if entry
            .capture_after
            .is_some_and(|id| id > packets.len() as u64)
        {
            return Err(format!("Entry {index} has an out-of-range capture anchor"));
        }
    }
    Ok(snapshots)
}

fn official_entries(session: &Session) -> Vec<bool> {
    let mut legacy_game = MatchType::Official;
    session
        .entries
        .iter()
        .map(|entry| {
            if !entry.undone {
                if entry.kind == EventKind::GoldenGoalStart {
                    legacy_game = MatchType::GoldenGoal;
                } else if entry.ends_golden_goal() {
                    legacy_game = MatchType::Official;
                }
            }
            entry.kind != EventKind::GoldenGoalStart
                && entry.game != Some(MatchType::GoldenGoal)
                && match session.match_type {
                    Some(MatchType::Official) => true,
                    Some(MatchType::GoldenGoal) => false,
                    Some(MatchType::Combined) => entry.game == Some(MatchType::Official),
                    None => legacy_game == MatchType::Official,
                }
        })
        .collect()
}

fn official_at(session: &Session, official: &[bool], existing: Option<usize>, id: u64) -> bool {
    if let Some(index) = existing {
        return official[index];
    }
    match session.match_type {
        Some(MatchType::Official) => true,
        Some(MatchType::GoldenGoal) => false,
        Some(MatchType::Combined) | None => {
            // The raw scope must also agree with an existing history anchor. The
            // following anchored event bounds where insertion belongs in history.
            let index = insertion_index(session, id).min(session.entries.len().saturating_sub(1));
            official
                .get(index)
                .copied()
                .unwrap_or(session.match_type.is_none())
        }
    }
}

fn closed_periods(
    session: &Session,
    official: &[bool],
    packets: &[Packet],
    snapshots: &[Option<Snapshot>],
) -> Vec<(u64, Option<u64>)> {
    session
        .entries
        .iter()
        .zip(official)
        .filter_map(|(end, &is_official)| {
            let bound = end.capture_after?;
            if !is_official || end.from_controller || end.kind != EventKind::GameEnd {
                return None;
            }
            // Undo/restore changes the visible history, not which packets were imported.
            let reopen = session
                .entries
                .iter()
                .zip(official)
                .filter_map(|(entry, &is_official)| {
                    let id = entry.capture_id?;
                    let packet = &packets[(id - 1) as usize];
                    (is_official
                        && id > bound
                        && packet.active
                        && packet.game == Some(MatchType::Official)
                        && snapshots[(id - 1) as usize]
                            .as_ref()
                            .is_some_and(|snapshot| matches!(snapshot.game_phase, 0 | 2 | 3)))
                    .then_some(id)
                })
                .min();
            Some((bound, reopen))
        })
        .collect()
}

fn insertion_index(session: &Session, id: u64) -> usize {
    session
        .entries
        .iter()
        .position(|entry| {
            (entry.from_controller && entry.capture_id.is_some_and(|next| next > id))
                || (!entry.from_controller && entry.capture_after.is_some_and(|after| after >= id))
        })
        .unwrap_or(session.entries.len())
}

fn history_changed(before: &Snapshot, after: &Snapshot) -> bool {
    (
        before.hulks_score,
        before.opponent_score,
        before.game_phase,
        before.state,
        before.first_half,
    ) != (
        after.hulks_score,
        after.opponent_score,
        after.game_phase,
        after.state,
        after.first_half,
    )
}

fn baseline(snapshot: &Snapshot, pre_game: bool) -> Statistics {
    Statistics {
        baseline: Some(StatisticsBaseline {
            penalties: snapshot.hulks_penalties,
            pre_game,
        }),
        penalties: Vec::new(),
        opponent_restart: false,
    }
}

fn derive(snapshot: &Snapshot, previous: Option<&Snapshot>) -> Statistics {
    let Some(previous) = previous else {
        return baseline(
            snapshot,
            snapshot.game_phase == 0
                && snapshot.first_half
                && snapshot.state == 0
                && snapshot.hulks_score == 0
                && snapshot.opponent_score == 0,
        );
    };
    Statistics {
        baseline: None,
        penalties: previous
            .hulks_penalties
            .iter()
            .zip(snapshot.hulks_penalties)
            .enumerate()
            .filter_map(|(index, (&before, after))| {
                (before != after).then_some(PenaltyChange {
                    player: index as u8 + 1,
                    before,
                    after,
                })
            })
            .collect(),
        opponent_restart: matches!(snapshot.game_phase, 0 | 2)
            && matches!(snapshot.set_play, 4 | 5)
            && snapshot.kicking_team == snapshot.opponent_number
            && (previous.set_play, previous.kicking_team)
                != (snapshot.set_play, snapshot.kicking_team),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Metadata;
    use chrono::{DateTime, NaiveDate};

    fn session() -> Session {
        Session::new_match(
            Metadata {
                event: "Synthetic fixture".into(),
                home: "HULKs".into(),
                away: "Opponent".into(),
                date: NaiveDate::from_ymd_opt(2026, 10, 4).unwrap(),
                label: "Rebuild test".into(),
                robots: std::array::from_fn(|_| String::new()),
            },
            MatchType::Official,
        )
    }

    fn packet(id: u64) -> Packet {
        let mut bytes = vec![0; 158];
        bytes[..4].copy_from_slice(b"RGme");
        bytes[4] = 20;
        bytes[5] = id as u8;
        bytes[6] = 5;
        bytes[12] = 1;
        bytes[13] = 24;
        bytes[14..16].copy_from_slice(&600_i16.to_le_bytes());
        bytes[18] = 24;
        bytes[88] = 5;
        Packet {
            id,
            received_at: DateTime::parse_from_rfc3339("2026-10-04T12:00:00.123456789+02:00")
                .unwrap()
                + chrono::Duration::milliseconds(id as i64 * 100),
            source: "127.0.0.1:3838".parse().unwrap(),
            true_data: false,
            accepted: true,
            baseline: id == 1,
            packet: bytes,
            game: Some(MatchType::Official),
            active: true,
        }
    }

    fn linked(session: &mut Session, packet: &Packet, kind: EventKind) -> usize {
        session.record(kind, packet.received_at);
        let index = session.entries.len() - 1;
        let entry = &mut session.entries[index];
        entry.from_controller = true;
        entry.gc_seconds_remaining = Some(600);
        entry.capture_id = Some(packet.id);
        entry.text = "GC received: original snapshot; secondary 00:00".into();
        index
    }

    fn stats(session: &Session, id: u64) -> &Statistics {
        session
            .entries
            .iter()
            .find(|entry| entry.capture_id == Some(id))
            .unwrap()
            .statistics
            .as_ref()
            .unwrap()
    }

    fn bytes(session: &Session) -> Vec<u8> {
        serde_json::to_vec(session).unwrap()
    }

    fn assert_idempotent(session: &mut Session, packets: &[Packet]) {
        let saved = bytes(session);
        assert_eq!(rebuild(session, packets).unwrap(), 0);
        assert_eq!(bytes(session), saved);
    }

    #[test]
    fn fills_only_missing_statistics_preserving_every_original_field() {
        let mut session = session();
        let mut packets: Vec<_> = (1..=3).map(packet).collect();
        packets[1].packet[28] = 2;
        packets[2].packet[28] = 5;
        session.record(EventKind::Custom, packets[0].received_at);
        session.entries[0].from_controller = true;
        linked(&mut session, &packets[0], EventKind::Custom);
        session.record(EventKind::Note, packets[0].received_at);
        session.entries[2].text = "My edited note".into();
        session.entries[2].capture_after = Some(1);
        let corrected = linked(&mut session, &packets[1], EventKind::GoalAway);
        let entry = &mut session.entries[corrected];
        entry.text = "Corrected by the observer".into();
        entry.timestamp += chrono::Duration::hours(2);
        entry.gc_seconds_remaining = Some(-17);
        entry.score_delta = Some([3, -1]);
        entry.message_budget = Some(9);
        entry.golden_goal_message_budget = Some(2);
        entry.undone = true;
        let already = linked(&mut session, &packets[2], EventKind::Custom);
        session.entries[already].statistics = Some(Statistics {
            baseline: None,
            penalties: vec![],
            opponent_restart: true,
        });
        let original = session.clone();
        assert_eq!(rebuild(&mut session, &packets).unwrap(), 2);
        assert!(stats(&session, 1).baseline.as_ref().unwrap().pre_game);
        assert_eq!(
            stats(&session, 2).penalties,
            vec![PenaltyChange {
                player: 1,
                before: 0,
                after: 2,
            }]
        );
        let mut without_backfill = session.clone();
        for (entry, old) in without_backfill.entries.iter_mut().zip(&original.entries) {
            entry.statistics = old.statistics.clone();
        }
        assert_eq!(without_backfill, original);
        assert_eq!(session.entries[already], original.entries[already]);
        assert_idempotent(&mut session, &packets);
    }

    #[test]
    fn inserts_penalty_only_updates_without_reordering_history_or_active_note() {
        let mut session = session();
        let mut packets: Vec<_> = (1..=4).map(packet).collect();
        packets[1].packet[28] = 2;
        packets[2].packet[28] = 5;
        packets[3].packet[28] = 5;
        session.record(EventKind::Note, packets[0].received_at);
        session.entries[0].text = "Old unanchored note".into();
        linked(&mut session, &packets[0], EventKind::Custom);
        session.start_note(packets[2].received_at);
        session.entries[2].capture_after = Some(3);
        session.entries[2].text = "Keep writing here".into();
        linked(&mut session, &packets[3], EventKind::Custom);
        let original = session.entries.clone();
        assert_eq!(rebuild(&mut session, &packets).unwrap(), 4);
        assert_eq!(
            session
                .entries
                .iter()
                .map(|entry| entry.capture_id)
                .collect::<Vec<_>>(),
            vec![None, Some(1), Some(2), Some(3), None, Some(4)]
        );
        assert_eq!(session.active_note, Some(4));
        assert_eq!(session.entries[0], original[0]);
        assert_eq!(session.entries[4], original[2]);
        assert_eq!(stats(&session, 2).penalties[0].after, 2);
        assert_eq!(stats(&session, 3).penalties[0].before, 2);
        assert_eq!(stats(&session, 3).penalties[0].after, 5);
        assert_eq!(session.entries[2].timestamp, packets[1].received_at);
        assert_eq!(session.entries[2].gc_seconds_remaining, Some(600));
        assert!(!session.markdown().contains("statistics backfill"));
        assert_idempotent(&mut session, &packets);
    }

    #[test]
    fn first_snapshot_is_a_baseline_not_an_invented_goal_penalty_or_restart() {
        let mut session = session();
        let mut packet = packet(1);
        packet.packet[10] = 3;
        packet.packet[11] = 5;
        packet.packet[13] = 5;
        packet.packet[22] = 3;
        packet.packet[28] = 2;
        packet.packet[31] = 5;
        assert_eq!(rebuild(&mut session, &[packet.clone()]).unwrap(), 1);
        let statistics = stats(&session, 1);
        assert!(!statistics.baseline.as_ref().unwrap().pre_game);
        assert_eq!(statistics.baseline.as_ref().unwrap().penalties[..2], [2, 5]);
        assert!(statistics.penalties.is_empty());
        assert!(!statistics.opponent_restart);
        assert_eq!(session.score(), (0, 0));
        assert_eq!(session.entries[0].kind, EventKind::Custom);
        assert_idempotent(&mut session, &[packet]);
    }

    #[test]
    fn pre_game_requires_normal_initial_first_half_and_zero_scores() {
        for (offset, value, expected) in [
            (9, 0, true),
            (9, 2, false),
            (9, 3, false),
            (10, 1, false),
            (12, 0, false),
            (22, 1, false),
            (92, 1, false),
        ] {
            let mut session = session();
            let mut packet = packet(1);
            packet.packet[offset] = value;
            rebuild(&mut session, &[packet]).unwrap();
            assert_eq!(
                stats(&session, 1).baseline.as_ref().unwrap().pre_game,
                expected
            );
        }
    }

    #[test]
    fn restart_tuple_changes_count_but_clock_only_packets_do_not_insert_entries() {
        let mut session = session();
        let mut packets: Vec<_> = (1..=7).map(packet).collect();
        for (index, (set_play, kicking_team)) in
            [(0, 24), (4, 5), (4, 5), (5, 5), (5, 24), (5, 5), (4, 5)]
                .into_iter()
                .enumerate()
        {
            packets[index].packet[11] = set_play;
            packets[index].packet[13] = kicking_team;
            packets[index].packet[14..16].copy_from_slice(&(600_i16 - index as i16).to_le_bytes());
        }
        packets[6].packet[9] = 2;
        linked(&mut session, &packets[6], EventKind::Custom);
        assert_eq!(rebuild(&mut session, &packets).unwrap(), 5);
        assert_eq!(
            session
                .entries
                .iter()
                .map(|entry| entry.capture_id.unwrap())
                .collect::<Vec<_>>(),
            vec![1, 2, 4, 6, 7]
        );
        for id in [2, 4, 6, 7] {
            assert!(stats(&session, id).opponent_restart);
        }
        assert_idempotent(&mut session, &packets);
    }

    #[test]
    fn timeout_preserves_penalty_coverage_without_counting_restarts() {
        let mut session = session();
        let mut packets: Vec<_> = (1..=4).map(packet).collect();
        packets[1].packet[9] = 3;
        packets[1].packet[11] = 5;
        packets[1].packet[13] = 5;
        packets[1].packet[28] = 2;
        packets[2].packet[9] = 3;
        packets[2].packet[11] = 5;
        packets[2].packet[13] = 5;
        packets[3].packet[11] = 5;
        packets[3].packet[13] = 5;
        for index in [0, 1, 3] {
            linked(&mut session, &packets[index], EventKind::Custom);
        }
        assert_eq!(rebuild(&mut session, &packets).unwrap(), 4);
        assert_eq!(stats(&session, 2).penalties[0].after, 2);
        assert_eq!(stats(&session, 3).penalties[0].after, 0);
        for id in 2..=4 {
            assert!(stats(&session, id).baseline.is_none());
            assert!(!stats(&session, id).opponent_restart);
        }
        assert_idempotent(&mut session, &packets);
    }

    #[test]
    fn source_feed_sequence_opponent_time_and_receiver_discontinuities_rebaseline() {
        for discontinuity in 0..6 {
            let mut session = session();
            let mut packets: Vec<_> = (1..=2).map(packet).collect();
            for packet in &mut packets {
                packet.packet[10] = 3;
            }
            packets[1].packet[28] = 5;
            match discontinuity {
                0 => packets[1].baseline = true,
                1 => packets[1].packet[5] = 9,
                2 => packets[1].source = "127.0.0.1:4000".parse().unwrap(),
                3 => {
                    packets[1].true_data = true;
                    packets[1].packet[..4].copy_from_slice(b"RGTD");
                }
                4 => packets[1].packet[88] = 6,
                _ => packets[1].received_at += chrono::Duration::seconds(6),
            }
            linked(&mut session, &packets[1], EventKind::Custom);
            rebuild(&mut session, &packets).unwrap();
            let statistics = stats(&session, 2);
            assert_eq!(statistics.baseline.as_ref().unwrap().penalties[0], 5);
            assert!(!statistics.baseline.as_ref().unwrap().pre_game);
            assert!(statistics.penalties.is_empty());
            assert_idempotent(&mut session, &packets);
        }
    }

    #[test]
    fn shootout_inactive_unknown_scope_and_golden_packets_break_coverage() {
        for excluded in 0..4 {
            let mut session = session();
            let mut packets: Vec<_> = (1..=3).map(packet).collect();
            for packet in &mut packets {
                packet.packet[10] = 3;
            }
            packets[2].packet[28] = 5;
            match excluded {
                0 => packets[1].packet[9] = 1,
                1 => packets[1].active = false,
                2 => packets[1].game = None,
                _ => packets[1].game = Some(MatchType::GoldenGoal),
            }
            rebuild(&mut session, &packets).unwrap();
            assert_eq!(session.entries.len(), 2);
            assert!(stats(&session, 3).baseline.is_some());
            assert!(stats(&session, 3).penalties.is_empty());
            assert_idempotent(&mut session, &packets);
        }
    }

    #[test]
    fn combined_scope_requires_raw_official_and_an_existing_official_anchor() {
        let mut session = session();
        session.match_type = Some(MatchType::Combined);
        let mut packets: Vec<_> = (1..=5).map(packet).collect();
        packets[0].game = Some(MatchType::GoldenGoal);
        packets[3].packet[28] = 2;
        packets[4].game = None;
        packets[4].packet[28] = 5;
        for (index, packet) in packets.iter().take(3).enumerate() {
            let entry = linked(&mut session, packet, EventKind::Custom);
            session.entries[entry].game = Some(if index == 2 {
                MatchType::Official
            } else {
                MatchType::GoldenGoal
            });
        }
        let before = session.entries[..2].to_vec();
        assert_eq!(rebuild(&mut session, &packets).unwrap(), 2);
        assert_eq!(session.entries[..2], before);
        assert!(stats(&session, 3).baseline.is_some());
        assert_eq!(stats(&session, 4).penalties[0].after, 2);
        assert_eq!(
            session.entries.last().unwrap().game,
            Some(MatchType::Official)
        );
        assert_idempotent(&mut session, &packets);
        let mut no_anchor = session.clone();
        no_anchor.entries.clear();
        assert_eq!(rebuild(&mut no_anchor, &packets).unwrap(), 0);
        no_anchor.match_type = Some(MatchType::GoldenGoal);
        no_anchor.metadata.away = "Invisibles".into();
        assert_eq!(rebuild(&mut no_anchor, &packets).unwrap(), 0);
    }

    #[test]
    fn manual_end_removal_and_restore_do_not_resurrect_closed_packets() {
        let mut session = session();
        let mut packets: Vec<_> = (1..=3).map(packet).collect();
        packets[1].packet[28] = 2;
        linked(&mut session, &packets[0], EventKind::Custom);
        session.record(EventKind::GameEnd, packets[0].received_at);
        session.entries[1].capture_after = Some(1);
        assert_eq!(rebuild(&mut session, &packets).unwrap(), 1);
        assert_eq!(session.entries.len(), 2);
        session.delete(1);
        assert_idempotent(&mut session, &packets);
        session.restore(1);
        assert_idempotent(&mut session, &packets);
    }

    #[test]
    fn later_link_reopens_only_from_that_packet_and_inactive_data_still_stays_out() {
        let mut session = session();
        let mut packets: Vec<_> = (1..=6).map(packet).collect();
        for packet in &mut packets {
            packet.packet[10] = 3;
        }
        packets[1].packet[28] = 2;
        packets[3].packet[28] = 5;
        packets[4].active = false;
        packets[5].packet[28] = 2;
        linked(&mut session, &packets[0], EventKind::Custom);
        session.record(EventKind::GameEnd, packets[0].received_at);
        session.entries[1].capture_after = Some(1);
        let reopened = linked(&mut session, &packets[3], EventKind::Custom);
        session.entries[reopened].undone = true;
        assert_eq!(rebuild(&mut session, &packets).unwrap(), 3);
        assert_eq!(
            session
                .entries
                .iter()
                .filter_map(|entry| entry.capture_id)
                .collect::<Vec<_>>(),
            vec![1, 4, 6]
        );
        assert!(session.entries[reopened].undone);
        assert!(stats(&session, 4).baseline.is_some());
        assert!(stats(&session, 6).baseline.is_some());
        assert_idempotent(&mut session, &packets);
    }

    #[test]
    fn missing_goal_or_phase_history_forces_partial_without_recreating_history() {
        for (offset, value) in [(22, 1), (10, 3), (12, 0), (9, 2)] {
            let mut session = session();
            let mut packets: Vec<_> = (1..=2).map(packet).collect();
            packets[1].packet[offset] = value;
            linked(&mut session, &packets[0], EventKind::Custom);
            session.entries[0].statistics = Some(baseline(
                &decode_capture(&packets[0].packet, false).unwrap().unwrap(),
                true,
            ));
            session.record(EventKind::GameEnd, packets[1].received_at);
            session.entries[1].capture_after = Some(2);
            session.entries[1].gc_seconds_remaining = Some(0);
            let original = session.entries.clone();
            assert_eq!(rebuild(&mut session, &packets).unwrap(), 1);
            assert_eq!(session.entries[0], original[0]);
            assert_eq!(session.entries[2], original[1]);
            assert_eq!(session.entries[1].kind, EventKind::Custom);
            assert!(!stats(&session, 2).baseline.as_ref().unwrap().pre_game);
            assert_eq!(session.score(), (0, 0));
            let markdown = session.markdown();
            assert!(markdown.contains("partial GC coverage"));
            assert!(markdown.contains("Goal share: unavailable"));
            assert!(!markdown.contains("statistics backfill"));
            assert_idempotent(&mut session, &packets);
        }
    }

    #[test]
    fn validation_errors_are_atomic() {
        for invalid in 0..9 {
            let mut session = session();
            let mut packets: Vec<_> = (1..=2).map(packet).collect();
            linked(&mut session, &packets[0], EventKind::Custom);
            match invalid {
                0 => packets[1].id = 1,
                1 => packets[1].id = 3,
                2 => session.entries[0].capture_id = Some(3),
                3 => session.entries[0].capture_after = Some(3),
                4 => packets[1].packet[0] = b'!',
                5 => packets[1].packet[18] = 6,
                6 => packets[1].true_data = true,
                7 => {
                    let entry = session.entries[0].clone();
                    session.entries.push(entry);
                }
                _ => session.entries[0].from_controller = false,
            }
            let before = bytes(&session);
            assert!(rebuild(&mut session, &packets).is_err(), "case {invalid}");
            assert_eq!(bytes(&session), before, "case {invalid}");
        }
    }

    #[test]
    fn rejected_raw_packets_need_not_decode_or_interrupt_the_selected_sequence() {
        let mut session = session();
        let mut packets: Vec<_> = (1..=3).map(packet).collect();
        packets[1].accepted = false;
        packets[1].packet = vec![0, 1, 2];
        packets[2].packet[5] = 2;
        packets[2].packet[28] = 5;
        assert_eq!(rebuild(&mut session, &packets).unwrap(), 2);
        assert!(stats(&session, 3).baseline.is_none());
        assert_eq!(stats(&session, 3).penalties[0].after, 5);
        assert_idempotent(&mut session, &packets);
    }
}
