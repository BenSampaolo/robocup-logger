use super::*;
use crate::test_support::{note, packet, session, time};

#[test]
fn team_perspective_and_reversible_scores_survive_serialization() {
    let mut session = session(MatchType::Official);
    session.metadata.home = "B-Human".into();
    session.metadata.away = "HTWK Robots".into();
    session.metadata.tracked_team = Some(14);
    session.validate().unwrap();
    assert!(!session.metadata.tracked_are_home());
    session.record(EventKind::GoalAway, time());
    assert_eq!(session.score(), (0, 1));
    session.entries[0].undone = true;
    assert_eq!(session.score(), (0, 0));
    let restored: Session = serde_json::from_slice(&serde_json::to_vec(&session).unwrap()).unwrap();
    assert_eq!(restored.metadata.tracked_team, Some(14));
    session.metadata.tracked_team = Some(24);
    assert!(session.validate().is_err());
}

#[test]
fn statistics_rebuild_is_idempotent_and_preserves_notes() {
    let mut session = session(MatchType::Official);
    session.record(EventKind::GameStart, time());
    session.entries[0].capture_id = Some(1);
    session.entries[0].from_controller = true;
    note(&mut session, "Keep this text");
    session.active_note = Some(1);
    let packets = [packet(1, 0, 0), packet(2, 3, 0)];
    crate::rebuild::rebuild(&mut session, &packets).unwrap();
    assert_eq!(
        session.entries[session.active_note.unwrap()].text,
        "Keep this text"
    );
    let before = serde_json::to_vec(&session).unwrap();
    crate::rebuild::rebuild(&mut session, &packets).unwrap();
    assert_eq!(serde_json::to_vec(&session).unwrap(), before);
    let mut invalid = packets.to_vec();
    invalid[1].id = 1;
    assert!(crate::rebuild::rebuild(&mut session, &invalid).is_err());
    assert_eq!(serde_json::to_vec(&session).unwrap(), before);
}
