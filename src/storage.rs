use crate::capture::{CaptureLog, RobotCaptureLog};
use crate::model::{MARKDOWN_MARKER, Session};
use chrono::Local;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub struct Store {
    session_path: PathBuf,
    markdown_path: PathBuf,
    _lock: SessionLock,
}

struct SessionLock(File);

impl Drop for SessionLock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
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
        atomic_write(&store.session_path, &json, false)?;
        atomic_write(&store.markdown_path, session.markdown().as_bytes(), false)?;
        Ok(store)
    }

    pub fn open(json_path: &Path) -> io::Result<(Self, Session)> {
        let (store, session) = Self::load(json_path)?;
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

    pub fn robot_capture_path(&self) -> PathBuf {
        self.session_path.with_extension("robots.jsonl")
    }

    pub fn prepare_robot_capture(&self, session: &Session) -> io::Result<RobotCaptureLog> {
        let owner = session
            .capture_session
            .as_deref()
            .ok_or_else(|| invalid_data("Start GC capture before preparing the robot archive"))?;
        let path = self.robot_capture_path();
        if !regular_file_exists(&path)? {
            self.save(session)?;
        }
        RobotCaptureLog::open(&path, owner)
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

pub(crate) fn regular_file_exists(path: &Path) -> io::Result<bool> {
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

fn lock_session(path: &Path) -> io::Result<SessionLock> {
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
    Ok(SessionLock(file))
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

pub(crate) fn atomic_write(path: &Path, contents: &[u8], replace: bool) -> io::Result<()> {
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

