use super::*;

#[test]
fn invalid_settings_and_conflicting_controls_cannot_overwrite_saved_settings() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    let mut settings = Settings::default();
    settings.save_to(&path).unwrap();
    assert_eq!(Settings::load_from(&path).unwrap(), settings);
    let before = fs::read(&path).unwrap();
    settings.keys.confirm = settings.keys.back.clone();
    assert!(settings.save_to(&path).is_err());
    assert_eq!(fs::read(&path).unwrap(), before);
    settings = Settings::default();
    settings.keys.confirm = vec!["Tab".into()];
    assert!(settings.validate().is_err());
    fs::write(&path, "{malformed").unwrap();
    assert!(Settings::default().save_to(&path).is_err());
    assert_eq!(fs::read_to_string(&path).unwrap(), "{malformed");
}
