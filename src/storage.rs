use crate::capture::CaptureLog;
use crate::model::{MARKDOWN_MARKER, Session};
use chrono::Local;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub struct Store {
    session_path: PathBuf,
    markdown_path: PathBuf,
    // Lock an inode that is never replaced by an atomic save or deleted on exit.
    _lock: File,
}

impl Store {
    pub fn create(output_dir: &Path, session: &Session) -> io::Result<Self> {
        session.validate().map_err(invalid_data)?;
        fs::create_dir_all(output_dir)?;
        let directory = output_dir.canonicalize()?;
        let label: String = session
            .metadata
            .label
            .chars()
            .take(80)
            .map(|c| {
                if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let label = label.trim_matches('_');
        let label = if label.is_empty() { "game" } else { label };
        let folder = format!(
            "{}_{}-{label}",
            session.metadata.date.format("%d-%m-%Y"),
            Local::now().format("%H-%M")
        );
        let directory = directory.join(&folder);
        fs::create_dir(&directory)?;
        let stem = format!("Log_{folder}");
        let session_path = directory.join(format!("{stem}.json"));
        let markdown_path = directory.join(format!("{stem}.md"));
        let lock = lock_session(&session_path.with_extension("lock"))?;
        for path in [&session_path, &markdown_path] {
            match fs::symlink_metadata(path) {
                Ok(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        format!(
                            "{} already exists; choose another label or reopen its JSON",
                            path.display()
                        ),
                    ));
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        let store = Self {
            session_path,
            markdown_path,
            _lock: lock,
        };
        let json = serde_json::to_vec_pretty(session).map_err(invalid_data)?;
        // No-clobber publication also protects against files created after the checks above.
        atomic_write(&store.session_path, &json, false)?;
        atomic_write(&store.markdown_path, session.markdown().as_bytes(), false)?;
        Ok(store)
    }

    pub fn open(json_path: &Path) -> io::Result<(Self, Session)> {
        let (store, session) = Self::load(json_path)?;
        // JSON is authoritative after a crash between the two file replacements.
        store.save(&session)?;
        Ok((store, session))
    }

    pub fn export(json_path: &Path) -> io::Result<PathBuf> {
        let (store, session) = Self::load(json_path)?;
        let replace_markdown = generated_markdown_exists(&store.markdown_path)?;
        atomic_write(
            &store.markdown_path,
            session.markdown().as_bytes(),
            replace_markdown,
        )?;
        Ok(store.markdown_path)
    }

    pub(crate) fn load(json_path: &Path) -> io::Result<(Self, Session)> {
        if json_path
            .extension()
            .is_none_or(|extension| extension != "json")
        {
            return Err(invalid_data("Open a .json session file"));
        }
        if !regular_file_exists(json_path)? {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "Session JSON does not exist",
            ));
        }
        let session_path = json_path.canonicalize()?;
        let lock = lock_session(&session_path.with_extension("lock"))?;
        let session: Session =
            serde_json::from_reader(File::open(&session_path)?).map_err(invalid_data)?;
        session.validate().map_err(invalid_data)?;
        let store = Self {
            markdown_path: session_path.with_extension("md"),
            session_path,
            _lock: lock,
        };
        Ok((store, session))
    }

    pub fn save(&self, session: &Session) -> io::Result<()> {
        session.validate().map_err(invalid_data)?;
        let replace_json = regular_file_exists(&self.session_path)?;
        let replace_markdown = generated_markdown_exists(&self.markdown_path)?;
        let json = serde_json::to_vec_pretty(session).map_err(invalid_data)?;
        atomic_write(&self.session_path, &json, replace_json)?;
        atomic_write(
            &self.markdown_path,
            session.markdown().as_bytes(),
            replace_markdown,
        )
    }

    pub fn capture_path(&self) -> PathBuf {
        self.session_path.with_extension("gc.jsonl")
    }

    pub fn prepare_capture(&self, session: &mut Session) -> io::Result<CaptureLog> {
        let path = self.capture_path();
        let exists = regular_file_exists(&path)?;
        let last_reference = session
            .entries
            .iter()
            .flat_map(|entry| [entry.capture_id, entry.capture_after])
            .flatten()
            .max();
        if !exists && last_reference.is_some() {
            return Err(invalid_data(
                "Capture archive is missing but session entries still reference it",
            ));
        }
        if session.capture_session.is_none() {
            if exists {
                return Err(invalid_data(
                    "Refusing to adopt an existing capture without a session identifier",
                ));
            }
            static NEXT_CAPTURE: AtomicU64 = AtomicU64::new(0);
            session.capture_session = Some(format!(
                "{}-{}-{}",
                Local::now().format("%Y%m%dT%H%M%S%.9f%z"),
                std::process::id(),
                NEXT_CAPTURE.fetch_add(1, Ordering::Relaxed),
            ));
        }
        if !exists {
            // Persist ownership before publishing the sidecar, including retries after a failed save.
            self.save(session)?;
        }
        let capture = CaptureLog::open(&path, session.capture_session.as_deref().unwrap())?;
        if last_reference.is_some_and(|id| id >= capture.next_id()) {
            return Err(invalid_data(
                "Session entries reference packets missing from the capture archive",
            ));
        }
        Ok(capture)
    }

    pub fn markdown_path(&self) -> &Path {
        &self.markdown_path
    }

    pub fn session_path(&self) -> &Path {
        &self.session_path
    }
}

fn invalid_data(error: impl ToString) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

fn regular_file_exists(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => Ok(true),
        Ok(_) => Err(invalid_data(format!(
            "Not a regular file: {}",
            path.display()
        ))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn lock_session(path: &Path) -> io::Result<File> {
    let exists = regular_file_exists(path)?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(!exists)
        .open(path)?;
    file.try_lock().map_err(|error| match error {
        std::fs::TryLockError::WouldBlock => io::Error::new(
            io::ErrorKind::WouldBlock,
            format!(
                "Session is already open in another logger: {}",
                path.display()
            ),
        ),
        std::fs::TryLockError::Error(error) => error,
    })?;
    Ok(file)
}

fn generated_markdown_exists(path: &Path) -> io::Result<bool> {
    if !regular_file_exists(path)? {
        return Ok(false);
    }
    let mut file = File::open(path)?;
    let mut marker = vec![0; MARKDOWN_MARKER.len()];
    if file.metadata()?.len() >= marker.len() as u64 {
        file.seek(SeekFrom::End(-(marker.len() as i64)))?;
        file.read_exact(&mut marker)?;
        if marker == MARKDOWN_MARKER.as_bytes() {
            return Ok(true);
        }
    }
    Err(invalid_data(format!(
        "Refusing to overwrite unrecognized Markdown: {}",
        path.display()
    )))
}

fn atomic_write(path: &Path, contents: &[u8], replace: bool) -> io::Result<()> {
    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
    let directory = path
        .parent()
        .ok_or_else(|| invalid_data("Output path has no parent"))?;
    let (temp_path, mut temp_file) = loop {
        let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let candidate = directory.join(format!(".robocup-logger-{}-{id}.tmp", std::process::id()));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => break (candidate, file),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    };
    let result = (|| {
        temp_file.write_all(contents)?;
        temp_file.sync_all()?;
        if replace {
            fs::rename(&temp_path, path)?;
        } else {
            // Unlike rename, a hard link atomically fails if the destination already exists.
            fs::hard_link(&temp_path, path)?;
            fs::remove_file(&temp_path)?;
        }
        File::open(directory)?.sync_all()
    })();
    if let Err(error) = result {
        if let Err(cleanup_error) = fs::remove_file(&temp_path)
            && cleanup_error.kind() != io::ErrorKind::NotFound
        {
            return Err(io::Error::new(
                error.kind(),
                format!(
                    "Writing {}: {error}; removing temporary file {}: {cleanup_error}",
                    path.display(),
                    temp_path.display(),
                ),
            ));
        }
        return Err(io::Error::new(
            error.kind(),
            format!("Writing {}: {error}", path.display()),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{EventKind, Metadata};
    use chrono::{DateTime, NaiveDate};
    use tempfile::tempdir;

    fn session() -> Session {
        let mut session = Session::new(Metadata {
            event: "hobbymesse".into(),
            home: "HULKs".into(),
            away: "HTWK".into(),
            date: NaiveDate::from_ymd_opt(2026, 10, 3).unwrap(),
            label: "1st_game".into(),
            robots: std::array::from_fn(|_| String::new()),
        });
        session.start_note(DateTime::parse_from_rfc3339("2026-10-03T12:34:56.123+02:00").unwrap());
        session.entries[0].text = "draft\nwith two lines".into();
        session
    }

    #[test]
    fn round_trip_preserves_draft_and_regenerates_missing_markdown() {
        let directory = tempdir().unwrap();
        let original = session();
        let before = Local::now();
        let store = Store::create(directory.path(), &original).unwrap();
        let after = Local::now();
        let json = store.session_path().to_path_buf();
        let markdown = store.markdown_path().to_path_buf();
        assert!([before, after].iter().any(|created_at| {
            markdown.file_name().unwrap()
                == format!("Log_03-10-2026_{}-1st_game.md", created_at.format("%H-%M")).as_str()
        }));
        assert_eq!(json, markdown.with_extension("json"));
        assert_eq!(
            json.parent().unwrap().parent().unwrap(),
            directory.path().canonicalize().unwrap()
        );
        assert_eq!(
            json.parent()
                .unwrap()
                .file_name()
                .unwrap()
                .to_str()
                .unwrap(),
            json.file_stem()
                .unwrap()
                .to_str()
                .unwrap()
                .strip_prefix("Log_")
                .unwrap()
        );
        assert!(markdown.with_extension("lock").exists());
        assert_eq!(fs::read_to_string(&markdown).unwrap(), original.markdown());
        drop(store);
        fs::remove_file(&markdown).unwrap();
        let (store, reopened) = Store::open(&json).unwrap();
        assert_eq!(reopened, original);
        assert_eq!(
            fs::read_to_string(store.markdown_path()).unwrap(),
            original.markdown()
        );
    }

    #[test]
    fn export_regenerates_deterministically_without_rewriting_legacy_json() {
        let directory = tempdir().unwrap();
        let json = directory.path().join("legacy.json");
        let original = session();
        let mut value = serde_json::to_value(&original).unwrap();
        value["entries"][0]
            .as_object_mut()
            .unwrap()
            .remove("gc_seconds_remaining");
        let bytes = serde_json::to_vec(&value).unwrap();
        fs::write(&json, &bytes).unwrap();
        let modified = fs::metadata(&json).unwrap().modified().unwrap();
        let markdown = Store::export(&json).unwrap();
        assert_eq!(markdown, json.canonicalize().unwrap().with_extension("md"));
        assert_eq!(fs::read_to_string(&markdown).unwrap(), original.markdown());
        fs::write(&markdown, format!("stale export\n{MARKDOWN_MARKER}")).unwrap();
        assert_eq!(Store::export(&json).unwrap(), markdown);
        assert_eq!(fs::read_to_string(&markdown).unwrap(), original.markdown());
        assert_eq!(fs::read(&json).unwrap(), bytes);
        assert_eq!(fs::metadata(&json).unwrap().modified().unwrap(), modified);
    }

    #[test]
    fn export_respects_session_locks_and_protects_unrecognized_markdown() {
        let directory = tempdir().unwrap();
        let store = Store::create(directory.path(), &session()).unwrap();
        let json = store.session_path().to_owned();
        let markdown = store.markdown_path().to_owned();
        let json_before = fs::read(&json).unwrap();
        let markdown_before = fs::read(&markdown).unwrap();
        assert_eq!(
            Store::export(&json).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        assert_eq!(fs::read(&markdown).unwrap(), markdown_before);
        drop(store);
        fs::write(&markdown, b"Unrelated handwritten notes").unwrap();
        assert!(
            Store::export(&json)
                .unwrap_err()
                .to_string()
                .contains("Refusing to overwrite")
        );
        assert_eq!(fs::read(&markdown).unwrap(), b"Unrelated handwritten notes");
        assert_eq!(fs::read(&json).unwrap(), json_before);
    }

    #[test]
    fn export_rejects_malformed_and_invalid_sessions_without_writing_markdown() {
        let directory = tempdir().unwrap();
        let json = directory.path().join("invalid.json");
        let mut invalid = session();
        invalid.active_note = Some(999);
        for bytes in [b"{broken".to_vec(), serde_json::to_vec(&invalid).unwrap()] {
            fs::write(&json, &bytes).unwrap();
            assert!(Store::export(&json).is_err());
            assert!(!json.with_extension("md").exists());
            assert_eq!(fs::read(&json).unwrap(), bytes);
        }
    }

    #[test]
    fn opening_legacy_filenames_preserves_paths() {
        let directory = tempdir().unwrap();
        let json = directory.path().join("Log_03-10-2026-1st_game.json");
        let original = session();
        fs::write(&json, serde_json::to_vec(&original).unwrap()).unwrap();
        let (store, reopened) = Store::open(&json).unwrap();
        assert_eq!(reopened, original);
        assert_eq!(store.session_path(), json.canonicalize().unwrap());
        assert_eq!(
            store.markdown_path(),
            store.session_path().with_extension("md")
        );
        assert_eq!(
            fs::read_to_string(store.markdown_path()).unwrap(),
            original.markdown()
        );
    }

    #[test]
    fn locks_survive_atomic_replacements_and_release_when_store_drops() {
        let directory = tempdir().unwrap();
        let original = session();
        let store = Store::create(directory.path(), &original).unwrap();
        let json = store.session_path().to_path_buf();
        store.save(&original).unwrap();
        let error = Store::open(&json).err().unwrap();
        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
        drop(store);
        assert!(Store::open(&json).is_ok());
    }

    #[test]
    fn filename_is_contained_and_collisions_never_overwrite() {
        let directory = tempdir().unwrap();
        let mut original = session();
        original.metadata.label = "../../outside/name".into();
        let now = Local::now();
        let store = Store::create(directory.path(), &original).unwrap();
        assert_eq!(
            store.session_path().parent().unwrap().parent().unwrap(),
            directory.path().canonicalize().unwrap()
        );
        let json = store.session_path().to_path_buf();
        let before = fs::read(&json).unwrap();
        drop(store);
        let next_folder = format!(
            "03-10-2026_{}-outside_name",
            (now + chrono::Duration::minutes(1)).format("%H-%M")
        );
        let next_directory = directory.path().join(&next_folder);
        fs::create_dir_all(&next_directory).unwrap();
        let next_minute_json = next_directory.join(format!("Log_{next_folder}.json"));
        fs::write(&next_minute_json, &before).unwrap();
        assert_eq!(
            Store::create(directory.path(), &original)
                .err()
                .unwrap()
                .kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(fs::read(&json).unwrap(), before);
        assert_eq!(fs::read(&next_minute_json).unwrap(), before);
    }

    #[test]
    fn markdown_without_json_is_not_overwritten() {
        let directory = tempdir().unwrap();
        let now = Local::now();
        // Cover a minute rollover between writing the fixture and creating the store.
        let markdown_paths = [now, now + chrono::Duration::minutes(1)].map(|created_at| {
            let folder = format!("03-10-2026_{}-1st_game", created_at.format("%H-%M"));
            let game_directory = directory.path().join(&folder);
            fs::create_dir(&game_directory).unwrap();
            game_directory.join(format!("Log_{folder}.md"))
        });
        for markdown in &markdown_paths {
            fs::write(markdown, "existing match notes").unwrap();
        }
        assert!(Store::create(directory.path(), &session()).is_err());
        for markdown in &markdown_paths {
            assert_eq!(
                fs::read_to_string(markdown).unwrap(),
                "existing match notes"
            );
            assert!(!markdown.with_extension("json").exists());
        }
    }

    #[test]
    fn capture_ownership_is_persisted_and_survives_reopening_flat_sessions() {
        let directory = tempdir().unwrap();
        let json = directory.path().join("legacy.json");
        fs::write(&json, serde_json::to_vec(&session()).unwrap()).unwrap();
        let (store, mut session) = Store::open(&json).unwrap();
        assert!(session.capture_session.is_none());
        let capture = store.prepare_capture(&mut session).unwrap();
        assert_eq!(capture.next_id(), 1);
        assert_eq!(
            store.capture_path(),
            json.canonicalize().unwrap().with_extension("gc.jsonl")
        );
        let on_disk: Session = serde_json::from_slice(&fs::read(&json).unwrap()).unwrap();
        assert_eq!(on_disk.capture_session, session.capture_session);
        let capture_before = fs::read(store.capture_path()).unwrap();
        drop(capture);
        drop(store);
        let (store, mut reopened) = Store::open(&json).unwrap();
        assert!(store.prepare_capture(&mut reopened).is_ok());
        assert_eq!(reopened.capture_session, session.capture_session);
        assert_eq!(fs::read(store.capture_path()).unwrap(), capture_before);
        assert_eq!(store.session_path(), json.canonicalize().unwrap());
    }

    #[test]
    fn capture_creation_waits_for_saved_ownership_and_reuses_id_after_failure() {
        let directory = tempdir().unwrap();
        let mut original = session();
        let store = Store::create(directory.path(), &original).unwrap();
        let json_before = fs::read(store.session_path()).unwrap();
        fs::write(store.markdown_path(), "foreign notes").unwrap();
        assert!(store.prepare_capture(&mut original).is_err());
        assert!(!store.capture_path().exists());
        assert_eq!(fs::read(store.session_path()).unwrap(), json_before);
        let id = original.capture_session.clone().unwrap();
        fs::write(store.markdown_path(), original.markdown()).unwrap();
        assert!(store.prepare_capture(&mut original).is_ok());
        assert_eq!(original.capture_session.as_deref(), Some(id.as_str()));
        let persisted: Session =
            serde_json::from_slice(&fs::read(store.session_path()).unwrap()).unwrap();
        assert_eq!(persisted.capture_session, Some(id));
    }

    #[test]
    fn capture_refuses_unowned_and_mismatched_existing_archives() {
        let directory = tempdir().unwrap();
        let mut original = session();
        let store = Store::create(directory.path(), &original).unwrap();
        let json_before = fs::read(store.session_path()).unwrap();
        CaptureLog::open(&store.capture_path(), "foreign-session").unwrap();
        let before = fs::read(store.capture_path()).unwrap();
        assert!(store.prepare_capture(&mut original).is_err());
        assert!(original.capture_session.is_none());
        assert_eq!(fs::read(store.session_path()).unwrap(), json_before);
        original.capture_session = Some("own-session".into());
        assert!(store.prepare_capture(&mut original).is_err());
        assert_eq!(fs::read(store.capture_path()).unwrap(), before);
    }

    #[test]
    fn missing_archives_and_missing_referenced_packets_are_rejected() {
        let directory = tempdir().unwrap();
        let mut original = session();
        original.capture_session = Some("own-session".into());
        let store = Store::create(directory.path(), &original).unwrap();
        let json_before = fs::read(store.session_path()).unwrap();
        for (capture_id, capture_after) in [(Some(1), None), (None, Some(1))] {
            original.entries[0].capture_id = capture_id;
            original.entries[0].capture_after = capture_after;
            assert!(store.prepare_capture(&mut original).is_err());
            assert!(!store.capture_path().exists());
            assert_eq!(fs::read(store.session_path()).unwrap(), json_before);
        }
        CaptureLog::open(&store.capture_path(), "own-session").unwrap();
        let before = fs::read(store.capture_path()).unwrap();
        assert!(store.prepare_capture(&mut original).is_err());
        assert_eq!(fs::read(store.capture_path()).unwrap(), before);
        assert_eq!(fs::read(store.session_path()).unwrap(), json_before);
    }

    #[test]
    fn creating_a_game_leaves_matching_old_flat_logs_untouched() {
        let directory = tempdir().unwrap();
        let now = Local::now();
        let old_paths = [now, now + chrono::Duration::minutes(1)].map(|created_at| {
            let path = directory.path().join(format!(
                "Log_03-10-2026_{}-1st_game.json",
                created_at.format("%H-%M")
            ));
            fs::write(&path, "existing flat session").unwrap();
            fs::write(path.with_extension("md"), "existing flat notes").unwrap();
            path
        });
        let store = Store::create(directory.path(), &session()).unwrap();
        assert_ne!(
            store.session_path().parent().unwrap(),
            directory.path().canonicalize().unwrap()
        );
        for old in old_paths {
            assert_eq!(fs::read(&old).unwrap(), b"existing flat session");
            assert_eq!(
                fs::read(old.with_extension("md")).unwrap(),
                b"existing flat notes"
            );
        }
    }

    #[test]
    fn load_does_not_rewrite_json_or_markdown() {
        let directory = tempdir().unwrap();
        let json = directory.path().join("legacy.json");
        let before = serde_json::to_vec(&session()).unwrap();
        fs::write(&json, &before).unwrap();
        let (store, _) = Store::load(&json).unwrap();
        assert_eq!(fs::read(&json).unwrap(), before);
        assert!(!store.markdown_path().exists());
        assert!(!store.capture_path().exists());
    }

    #[test]
    fn atomic_publication_does_not_clobber_a_late_collision() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("existing.json");
        fs::write(&path, "existing session").unwrap();
        assert_eq!(
            atomic_write(&path, b"new session", false)
                .unwrap_err()
                .kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(fs::read(&path).unwrap(), b"existing session");
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_game_folders_and_locks_are_not_followed() {
        use std::os::unix::fs::symlink;
        let directory = tempdir().unwrap();
        let target = directory.path().join("target");
        fs::create_dir(&target).unwrap();
        let now = Local::now();
        for created_at in [now, now + chrono::Duration::minutes(1)] {
            let folder = directory.path().join(format!(
                "03-10-2026_{}-1st_game",
                created_at.format("%H-%M")
            ));
            symlink(&target, &folder).unwrap();
        }
        assert!(Store::create(directory.path(), &session()).is_err());
        assert_eq!(fs::read_dir(&target).unwrap().count(), 0);
        let json = directory.path().join("legacy.json");
        fs::write(&json, serde_json::to_vec(&session()).unwrap()).unwrap();
        let locked_target = directory.path().join("foreign-lock");
        fs::write(&locked_target, "foreign lock contents").unwrap();
        symlink(&locked_target, json.with_extension("lock")).unwrap();
        assert!(Store::load(&json).is_err());
        assert_eq!(fs::read(&locked_target).unwrap(), b"foreign lock contents");
    }

    #[test]
    fn corrections_keep_paths_and_open_repairs_a_stale_export() {
        let directory = tempdir().unwrap();
        let mut original = session();
        let store = Store::create(directory.path(), &original).unwrap();
        let json = store.session_path().to_path_buf();
        let markdown = store.markdown_path().to_path_buf();
        original.metadata.label = "corrected-label".into();
        original.record(EventKind::GoalAway, original.entries[0].timestamp);
        store.save(&original).unwrap();
        assert_eq!(store.session_path(), json);
        assert_eq!(store.markdown_path(), markdown);
        fs::write(&markdown, session().markdown()).unwrap();
        drop(store);
        let (store, reopened) = Store::open(&json).unwrap();
        assert_eq!(reopened.score(), (0, 1));
        assert_eq!(
            fs::read_to_string(store.markdown_path()).unwrap(),
            original.markdown()
        );
    }

    #[test]
    fn rejects_malformed_json_and_invalid_session_state() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("invalid.json");
        fs::write(&path, "not json").unwrap();
        assert!(Store::open(&path).is_err());
        let mut invalid = session();
        invalid.active_note = Some(99);
        fs::write(&path, serde_json::to_vec(&invalid).unwrap()).unwrap();
        assert!(Store::open(&path).is_err());
        invalid.active_note = None;
        invalid.entries[0].text = "unsafe\u{1b}[2J".into();
        fs::write(&path, serde_json::to_vec(&invalid).unwrap()).unwrap();
        assert!(Store::open(&path).is_err());
        let mut unexpected_field = serde_json::to_value(session()).unwrap();
        unexpected_field["path"] = "../other".into();
        fs::write(&path, serde_json::to_vec(&unexpected_field).unwrap()).unwrap();
        assert!(Store::open(&path).is_err());
    }

    #[test]
    fn refuses_foreign_markdown_on_save_and_open() {
        let directory = tempdir().unwrap();
        let original = session();
        let store = Store::create(directory.path(), &original).unwrap();
        let json = store.session_path().to_path_buf();
        let markdown = store.markdown_path().to_path_buf();
        fs::write(&markdown, "unrelated file").unwrap();
        assert!(store.save(&original).is_err());
        drop(store);
        assert!(Store::open(&json).is_err());
        assert_eq!(fs::read_to_string(markdown).unwrap(), "unrelated file");
    }

    #[cfg(unix)]
    #[test]
    fn refuses_symlinked_session_and_export() {
        use std::os::unix::fs::symlink;
        let directory = tempdir().unwrap();
        let original = session();
        let target = directory.path().join("target.json");
        fs::write(&target, serde_json::to_vec(&original).unwrap()).unwrap();
        let link = directory.path().join("linked.json");
        symlink(&target, &link).unwrap();
        assert!(Store::open(&link).is_err());
        let markdown = target.with_extension("md");
        symlink(&target, &markdown).unwrap();
        assert!(Store::open(&target).is_err());
        assert_eq!(
            fs::read(&target).unwrap(),
            serde_json::to_vec(&original).unwrap()
        );
    }
}
