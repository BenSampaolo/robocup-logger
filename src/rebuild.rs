use std::collections::HashMap;

use crate::{
    capture::Packet,
    game_controller::{Snapshot, decode_capture},
    model::{Entry, EventKind, MatchType, Session},
    statistics::Statistics,
};

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
        let missing_history = existing.is_none()
            && previous.is_some_and(|(_, before)| history_changed(before, snapshot));
        let mut statistics =
            Statistics::from_snapshots(snapshot, continuous.map(|(_, snapshot)| snapshot));
        if missing_history {
            statistics = Statistics::baseline(snapshot, false);
        }
        previous = Some((packet, snapshot));

        if let Some(index) = existing {
            if session.entries[index].statistics.is_none() {
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
                    postgame: None,
                    game: (session.match_type == Some(MatchType::Combined))
                        .then_some(MatchType::Official),
                    timestamp: packet.received_at,
                    gc_seconds_remaining: Some(snapshot.seconds_remaining),
                    kind: EventKind::Custom,
                    text: format!("GC received: statistics backfill; {}", snapshot.summary()),
                    undone: false,
                    from_controller: true,
                    controller_description_edited: false,
                    message_budget: Some(snapshot.tracked_message_budget),
                    golden_goal_message_budget: None,
                    score_delta: None,
                    statistics: Some(statistics),
                    capture_id: Some(packet.id),
                    capture_after: None,
                },
            );
            for position in [&mut candidate.active_note, &mut candidate.postgame_position] {
                if let Some(position) = position
                    && *position >= index
                {
                    *position += 1;
                }
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
                decode_capture(
                    &packet.packet,
                    packet.true_data,
                    session.metadata.tracked_team_number(),
                )
                .map_err(|error| format!("Capture packet {}: {error}", packet.id))?
                .ok_or_else(|| {
                    format!(
                        "Accepted capture packet {} does not contain tracked team #{}",
                        packet.id,
                        session.metadata.tracked_team_number()
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
        before.tracked_score,
        before.opponent_score,
        before.game_phase,
        before.state,
        before.first_half,
    ) != (
        after.tracked_score,
        after.opponent_score,
        after.game_phase,
        after.state,
        after.first_half,
    )
}
