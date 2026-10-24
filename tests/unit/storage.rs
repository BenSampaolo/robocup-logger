use super::*;
use crate::{model::MatchType, test_support::*};

#[test]
fn saved_session_is_exclusive_durable_and_reopenable() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = session(MatchType::Official);
    note(&mut session, "Robot fell");
    let store = Store::create(dir.path(), &session).unwrap();
    let path = store.session_path().to_owned();
    assert!(Store::load(&path).is_err());
    let inherited = store._lock.0.try_clone().unwrap();
    drop(store);
    let (store, reopened) = Store::open(&path).unwrap();
    assert_eq!(
        serde_json::to_value(&session).unwrap(),
        serde_json::to_value(reopened).unwrap()
    );
    drop(inherited);
    assert!(Store::load(&path).is_err());
    assert_eq!(
        fs::read_to_string(store.markdown_path()).unwrap(),
        session.markdown()
    );
}

#[test]
fn failed_saves_preserve_authoritative_data_and_foreign_files() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = session(MatchType::Official);
    let store = Store::create(dir.path(), &session).unwrap();
    let before = fs::read(store.session_path()).unwrap();
    session.postgame_position = Some(99);
    assert!(store.save(&session).is_err());
    assert_eq!(fs::read(store.session_path()).unwrap(), before);
    session.postgame_position = None;
    fs::write(store.markdown_path(), "user document").unwrap();
    assert!(store.save(&session).is_err());
    assert_eq!(
        fs::read_to_string(store.markdown_path()).unwrap(),
        "user document"
    );
    let link = dir.path().join("link.json");
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(store.session_path(), &link).unwrap();
        assert!(Store::load(&link).is_err());
    }
}

#[test]
fn capture_ownership_is_saved_before_packets_and_missing_archives_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = session(MatchType::Official);
    let store = Store::create(dir.path(), &session).unwrap();
    let mut capture = store.prepare_capture(&mut session).unwrap();
    let saved: Session = serde_json::from_slice(&fs::read(store.session_path()).unwrap()).unwrap();
    assert_eq!(saved.capture_session, session.capture_session);
    assert!(saved.capture_session.is_some());
    capture.append(&[packet(1, 0, 0)]).unwrap();
    note(&mut session, "linked");
    session.entries[0].capture_id = Some(1);
    drop(capture);
    fs::remove_file(store.capture_path()).unwrap();
    assert!(store.prepare_capture(&mut session).is_err());
}
