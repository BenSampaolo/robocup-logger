use super::*;
use crate::test_support::{key, saved, time};

#[test]
fn detected_gc_setup_requires_team_choice_and_keeps_defaults_without_creating_a_match() {
    let settings = Settings {
        event: "Tournament".into(),
        ..Settings::default()
    };
    let mut app = App::new_with_settings(None, &settings).unwrap();
    app.offer_detected_teams(Some((101, 102)));
    let Mode::Form(form) = &app.mode else {
        panic!("expected setup")
    };
    assert_eq!(form.detected_teams.as_ref().unwrap().selected, 0);
    app.key(key(KeyCode::Enter), time()).unwrap();
    let Mode::Form(form) = &app.mode else {
        panic!("expected setup")
    };
    assert!(form.detected_teams.is_none());
    assert_eq!(form.fields[1].value(), settings.home);
    app.offer_detected_teams(None);
    let Mode::Form(form) = &app.mode else {
        panic!("expected setup")
    };
    assert!(form.detected_teams.is_none());
    app.offer_detected_teams(Some((101, 102)));
    for code in [KeyCode::Down, KeyCode::Down, KeyCode::Enter] {
        app.key(key(code), time()).unwrap();
    }
    let Mode::Form(form) = &app.mode else {
        panic!("expected setup")
    };
    assert_eq!(form.fields[1].value(), "foom bots");
    assert_eq!(form.fields[2].value(), "FC Isar robotics");
    assert_eq!(form.match_type, Some(MatchType::Official));
    assert_eq!(form.fields[0].value(), settings.event);
    for (field, robot) in form.fields[5..10].iter().zip(&settings.robots) {
        assert_eq!(field.value(), robot);
    }
    assert!(app.session.is_none() && app.store.is_none() && app.controller.is_none());
}

#[test]
fn live_notes_keep_their_timestamp_and_failed_saves_block_exit() {
    let (_dir, mut app) = saved(MatchType::Official);
    app.key(key(KeyCode::Enter), time()).unwrap();
    app.paste("Robot fell");
    app.persist();
    assert!(app.session.as_ref().unwrap().active_note.is_some());
    app.key(key(KeyCode::Esc), time() + chrono::Duration::seconds(20))
        .unwrap();
    let entry = &app.session.as_ref().unwrap().entries[0];
    assert_eq!(entry.timestamp, time());
    assert_eq!(entry.text, "Robot fell");
    std::fs::write(app.store.as_ref().unwrap().markdown_path(), "foreign file").unwrap();
    app.dirty = true;
    assert!(!app.request_quit());
    assert!(app.save_error.is_some());
    assert_eq!(app.session.as_ref().unwrap().entries[0].text, "Robot fell");
}

#[test]
fn event_removal_is_confirmed_visible_and_reversible() {
    let (_dir, mut app) = saved(MatchType::Official);
    app.perform_action(Action::Mark(EventKind::GoalHome), time());
    app.perform_action(Action::Delete, time());
    assert!(matches!(app.mode, Mode::Delete { selected: 0, .. }));
    app.key(key(KeyCode::Enter), time()).unwrap();
    assert_eq!(app.session.as_ref().unwrap().score(), (1, 0));
    app.perform_action(Action::Delete, time());
    app.key(key(KeyCode::Down), time()).unwrap();
    app.controls_visible = false;
    app.key(key(KeyCode::Enter), time()).unwrap();
    assert_eq!(app.session.as_ref().unwrap().score(), (1, 0));
    app.controls_visible = true;
    app.key(key(KeyCode::Enter), time()).unwrap();
    assert_eq!(app.session.as_ref().unwrap().score(), (0, 0));
    app.perform_action(Action::Restore, time());
    assert_eq!(app.session.as_ref().unwrap().score(), (1, 0));
}

#[test]
fn reconnect_requires_visible_confirmation_and_preserves_the_log() {
    let (_dir, mut app) = saved(MatchType::Official);
    let mut receiver = Receiver::for_test();
    receiver.source = Some("127.0.0.1:12345".parse().unwrap());
    app.controller = Some(receiver);
    let before = serde_json::to_vec(app.session.as_ref().unwrap()).unwrap();
    app.perform_action(Action::ReconnectController, time());
    assert!(matches!(
        app.mode,
        Mode::ReconnectController { selected: 0 }
    ));
    app.key(key(KeyCode::Enter), time()).unwrap();
    assert!(app.controller.as_ref().unwrap().source.is_some());
    app.perform_action(Action::ReconnectController, time());
    app.key(key(KeyCode::Down), time()).unwrap();
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(20, 8)).unwrap();
    terminal
        .draw(|frame| crate::ui::draw(frame, &mut app))
        .unwrap();
    app.key(key(KeyCode::Enter), time()).unwrap();
    assert!(app.controller.as_ref().unwrap().source.is_some());
    app.controls_visible = true;
    app.key(key(KeyCode::Enter), time()).unwrap();
    assert!(app.controller.as_ref().unwrap().source.is_none());
    assert_eq!(
        serde_json::to_vec(app.session.as_ref().unwrap()).unwrap(),
        before
    );
}
