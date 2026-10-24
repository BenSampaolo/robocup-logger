use super::*;
use crate::test_support::{note, session};

#[test]
fn transcript_preserves_audio_and_groups_revisits_without_mutating_snapshots() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = session(MatchType::Official);
    note(&mut session, "Original note");
    let store = Store::create(dir.path(), &session).unwrap();
    let directory = create(&store, &session).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
    let snapshot = NoteSnapshot::from_entry(0, &session.entries[0], &session);
    let markers = [
        (0.0, Some(snapshot.clone())),
        (1.0, None),
        (2.0, Some(snapshot)),
    ];
    let timeline = markers
        .into_iter()
        .map(|(seconds, note)| serde_json::to_string(&Marker { seconds, note }).unwrap() + "\n")
        .collect::<String>();
    let timeline_path = directory.join("timeline.jsonl");
    std::fs::write(&timeline_path, &timeline).unwrap();
    std::fs::write(directory.join("audio.caf"), "full recording").unwrap();
    session.entries[0].text = "Edited later".into();
    let transcript = Transcript {
        duration_seconds: 3.0,
        segments: vec![Segment {
            start: 0.5,
            end: 2.5,
            text: "Spoken discussion".into(),
        }],
    };
    publish(&directory, &transcript).unwrap();
    let restored = read_transcript_data(&directory).unwrap();
    assert_eq!(restored.markers.len(), 3);
    assert_eq!(
        restored.markers[2].note.as_ref().unwrap().text,
        "Original note"
    );
    let markdown = std::fs::read_to_string(directory.join("transcript.md")).unwrap();
    assert!(markdown.contains("Original note"));
    assert!(markdown.matches("Spoken discussion").count() >= 2);
    assert_eq!(std::fs::read_to_string(&timeline_path).unwrap(), timeline);
    assert_eq!(
        std::fs::read_to_string(directory.join("audio.caf")).unwrap(),
        "full recording"
    );
    let before = std::fs::read(directory.join("transcript.json")).unwrap();
    let invalid = Transcript {
        duration_seconds: f64::NAN,
        segments: vec![],
    };
    assert!(publish(&directory, &invalid).is_err());
    assert_eq!(
        std::fs::read(directory.join("transcript.json")).unwrap(),
        before
    );
    std::fs::write(&timeline_path, "{truncated").unwrap();
    assert!(publish(&directory, &transcript).is_err());
    assert_eq!(
        std::fs::read(directory.join("transcript.json")).unwrap(),
        before
    );
}
