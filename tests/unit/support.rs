use crate::{
    app::App,
    capture::Packet,
    model::{EventKind, MatchType, Metadata, Session},
    storage::Store,
};
use chrono::{DateTime, FixedOffset};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

pub fn time() -> DateTime<FixedOffset> {
    "2026-01-01T12:00:00+00:00".parse().unwrap()
}
pub fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}
pub fn session(kind: MatchType) -> Session {
    Session::new_match(
        Metadata {
            event: "Fixture".into(),
            home: "HULKs".into(),
            away: "HTWK Robots".into(),
            tracked_team: Some(24),
            date: time().date_naive(),
            label: "fixture".into(),
            robots: std::array::from_fn(|i| format!("Robot {}", i + 1)),
        },
        kind,
    )
}
pub fn saved(kind: MatchType) -> (tempfile::TempDir, App) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::create(dir.path(), &session(kind)).unwrap();
    let path = store.session_path().to_owned();
    drop(store);
    (dir, App::new(Some(&path)).unwrap())
}
pub fn note(session: &mut Session, text: &str) {
    session.record(EventKind::Note, time());
    session.entries.last_mut().unwrap().text = text.into();
}
pub fn wire(sequence: u8, opponent: u8, state: u8, score: u8) -> Vec<u8> {
    let mut bytes = vec![0; 158];
    bytes[..4].copy_from_slice(b"RGme");
    bytes[4] = 20;
    bytes[5] = sequence;
    bytes[6] = 5;
    bytes[10] = state;
    bytes[12] = 1;
    bytes[13] = 24;
    bytes[14..16].copy_from_slice(&600_i16.to_le_bytes());
    bytes[18] = 24;
    bytes[22] = score;
    bytes[88] = opponent;
    bytes
}
pub fn packet(id: u64, state: u8, score: u8) -> Packet {
    Packet {
        id,
        received_at: time() + chrono::Duration::seconds(id as i64),
        source: "127.0.0.1:12345".parse().unwrap(),
        true_data: false,
        accepted: true,
        baseline: id == 1,
        packet: wire(id as u8, 14, state, score),
        game: Some(MatchType::Official),
        active: true,
    }
}
