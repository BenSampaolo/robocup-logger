use super::*;
use crate::test_support::{key, note, session};

#[test]
fn recording_is_opt_in_and_hidden_confirmation_cannot_start_it() {
    let dir = tempfile::tempdir().unwrap();
    let session = session(MatchType::Official);
    let store = Store::create(dir.path(), &session).unwrap();
    let mut app = Postgame::new(session);
    assert_eq!(app.menu_selected, 0);
    app.menu_selected = 1;
    let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(20, 8)).unwrap();
    terminal.draw(|frame| app.draw(frame)).unwrap();
    app.key(key(KeyCode::Enter), &store);
    assert_eq!(app.menu, Some(Menu::RecordingChoice));
    assert!(!app.audio.busy());
    app.layout_ready = true;
    app.menu_selected = 0;
    app.key(key(KeyCode::Enter), &store);
    assert!(app.menu.is_none());
    assert!(!app.audio.busy());
}

#[test]
fn action_items_are_saved_and_finish_never_bypasses_failed_saves() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = session(MatchType::Official);
    note(&mut session, "Fell & stayed down");
    let store = Store::create(dir.path(), &session).unwrap();
    let mut app = Postgame::new(session);
    app.key(key(KeyCode::Enter), &store);
    app.open_menu(Menu::Actions);
    app.key(key(KeyCode::Enter), &store);
    assert_eq!(app.links().len(), 1);
    assert!(app.links()[0].contains("Fell%20%26%20stayed%20down"));
    app.open_menu(Menu::FinishWithTabs);
    assert_eq!(app.menu_selected, 0);
    assert!(app.key(key(KeyCode::Enter), &store).is_none());
    assert!(app.menu.is_none());
    std::fs::write(store.markdown_path(), "foreign file").unwrap();
    app.dirty = true;
    app.open_menu(Menu::FinishWithTabs);
    app.key(key(KeyCode::Down), &store);
    assert!(app.key(key(KeyCode::Enter), &store).is_none());
    assert!(app.error.is_some());
}
