use std::{
    fs::{self, File, OpenOptions},
    io::{self, BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver},
    thread::{self, JoinHandle},
    time::Instant,
};

use serde::Deserialize;

use crate::{
    meeting::{self, NoteSnapshot, Transcript},
    model::Session,
    settings::Settings,
    storage::Store,
};

struct Helper {
    directory: PathBuf,
    executable: PathBuf,
}

impl Helper {
    fn compile() -> io::Result<Self> {
        if !cfg!(target_os = "macos") {
            return Err(io::Error::other(
                "Recording and sandboxed local transcription require macOS.",
            ));
        }
        let directory =
            std::env::temp_dir().join(format!("robocup-audio-{}-{}", std::process::id(), nonce()));
        fs::create_dir(&directory)?;
        let helper = Self {
            executable: directory.join("postgame-audio"),
            directory,
        };
        let source = helper.directory.join("postgame-audio.swift");
        let plist = helper.directory.join("Info.plist");
        fs::write(&source, include_str!("../native/postgame-audio.swift"))?;
        fs::write(&plist, include_str!("../native/Info.plist"))?;
        let output = Command::new("/usr/bin/swiftc")
            .args([
                "-parse-as-library",
                "-O",
                "-framework",
                "AVFoundation",
                "-Xlinker",
                "-sectcreate",
                "-Xlinker",
                "__TEXT",
                "-Xlinker",
                "__info_plist",
                "-Xlinker",
            ])
            .arg(&plist)
            .arg(&source)
            .arg("-o")
            .arg(&helper.executable)
            .output()?;
        if !output.status.success() {
            return Err(io::Error::other(format!(
                "Cannot build the local audio helper (Apple command-line tools / Swift required): {}",
                String::from_utf8_lossy(&output.stderr)
                    .chars()
                    .take(1600)
                    .collect::<String>()
            )));
        }
        Ok(helper)
    }
}

impl Drop for Helper {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

#[derive(Deserialize)]
#[serde(tag = "event", rename_all = "lowercase")]
enum RecorderEvent {
    Ready,
    Stopped { seconds: f64 },
}

struct Recorder {
    child: Child,
    input: Option<ChildStdin>,
    events: Receiver<io::Result<RecorderEvent>>,
    directory: PathBuf,
    errors: PathBuf,
    started: Option<Instant>,
    stopping: bool,
    cancelled: bool,
    transcribe_after_stop: bool,
}

impl Recorder {
    fn spawn(helper: &Helper, directory: PathBuf, note: Option<NoteSnapshot>) -> io::Result<Self> {
        let errors = directory.join("recording.stderr");
        let stderr = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&errors)?;
        let mut child = Command::new(&helper.executable)
            .arg("record")
            .arg(&directory)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(stderr)
            .spawn()?;
        let input = child.stdin.take();
        let stdout = child.stdout.take().unwrap();
        let (sender, events) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let event =
                    line.and_then(|line| serde_json::from_str(&line).map_err(io::Error::other));
                let failed = event.is_err();
                if sender.send(event).is_err() || failed {
                    break;
                }
            }
        });
        let mut recorder = Self {
            child,
            input,
            events,
            directory,
            errors,
            started: None,
            stopping: false,
            cancelled: false,
            transcribe_after_stop: true,
        };
        recorder.send(serde_json::json!({"command": "mark", "note": note}))?;
        Ok(recorder)
    }

    fn send(&mut self, command: serde_json::Value) -> io::Result<()> {
        let input = self
            .input
            .as_mut()
            .ok_or_else(|| io::Error::other("Recorder input is closed"))?;
        serde_json::to_writer(&mut *input, &command)?;
        input.write_all(b"\n")?;
        input.flush()
    }

    fn stop(&mut self) -> io::Result<()> {
        if self.stopping {
            return Ok(());
        }
        self.stopping = true;
        if self.started.is_none() {
            // Cancelling during a permission prompt must not activate the microphone later.
            self.cancelled = true;
            self.child.kill()?;
        } else {
            self.send(serde_json::json!({"command": "stop"}))?;
            self.input.take();
        }
        Ok(())
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        self.input.take();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct TranscriptionJob {
    child: Child,
    directory: PathBuf,
    output: PathBuf,
    errors: PathBuf,
}

impl Drop for TranscriptionJob {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct PendingRecording {
    session: Session,
    note: Option<NoteSnapshot>,
}

#[derive(Default)]
pub struct Audio {
    settings: Settings,
    helper: Option<Helper>,
    preparing: Option<JoinHandle<io::Result<Helper>>>,
    pending: Option<PendingRecording>,
    recorder: Option<Recorder>,
    transcription: Option<TranscriptionJob>,
    latest: Option<PathBuf>,
    pub error: Option<String>,
    pub notice: Option<String>,
}

impl Audio {
    pub fn with_settings(settings: &Settings) -> Self {
        let mut audio = Self::default();
        audio.settings = settings.clone();
        audio
    }

    pub fn busy(&self) -> bool {
        self.preparing.is_some()
            || self.pending.is_some()
            || self.recorder.is_some()
            || self.transcription.is_some()
    }

    pub fn recording(&self) -> bool {
        self.recorder.is_some() || self.pending.is_some()
    }

    pub fn starting_recording(&self) -> bool {
        self.pending.is_some()
            || self
                .recorder
                .as_ref()
                .is_some_and(|recorder| recorder.started.is_none() && !recorder.stopping)
    }

    pub fn status(&self) -> String {
        if self.preparing.is_some() {
            return "Preparing local audio helper (mic OFF)".into();
        }
        if let Some(recorder) = &self.recorder {
            if recorder.stopping {
                return "Stopping recording…".into();
            }
            return recorder
                .started
                .map(|time| {
                    format!(
                        "REC {:02}:{:02} · MacBook microphone",
                        time.elapsed().as_secs() / 60,
                        time.elapsed().as_secs() % 60
                    )
                })
                .unwrap_or_else(|| "Waiting for microphone permission…".into());
        }
        if self.transcription.is_some() {
            return "Transcribing locally · microphone OFF".into();
        }
        "Microphone OFF".into()
    }

    pub fn start(&mut self, store: &Store, session: &Session, note: Option<NoteSnapshot>) {
        if self.busy() {
            return;
        }
        self.error = None;
        self.notice = None;
        self.pending = Some(PendingRecording {
            session: session.clone(),
            note,
        });
        self.prepare(store);
    }

    fn prepare(&mut self, store: &Store) {
        if self.helper.is_some() {
            self.start_pending(store);
        } else {
            self.preparing = Some(thread::spawn(Helper::compile));
        }
    }

    fn start_pending(&mut self, store: &Store) {
        let Some(pending) = self.pending.take() else {
            return;
        };
        let result = meeting::create(store, &pending.session).and_then(|directory| {
            let directory = directory.canonicalize()?;
            let recorder = Recorder::spawn(
                self.helper.as_ref().unwrap(),
                directory.clone(),
                pending.note,
            )?;
            self.recorder = Some(recorder);
            Ok(())
        });
        if let Err(error) = result {
            self.error = Some(error.to_string());
        }
    }

    pub fn mark(&mut self, note: Option<NoteSnapshot>) {
        if let Some(pending) = &mut self.pending {
            pending.note = note;
        } else if let Some(recorder) = &mut self.recorder
            && !recorder.stopping
            && let Err(error) = recorder.send(serde_json::json!({"command": "mark", "note": note}))
        {
            self.error = Some(format!("Could not record the note boundary: {error}"));
            let _ = recorder.stop();
        }
    }

    pub fn finish(&mut self) {
        if self.pending.take().is_some() {
            self.notice = Some("Recording cancelled before microphone startup.".into());
        }
        if let Some(recorder) = &mut self.recorder
            && let Err(error) = recorder.stop()
        {
            self.error = Some(format!("Could not stop recording: {error}"));
        }
    }

    pub fn cancel_transcription(&mut self) {
        let pending = if let Some(recorder) = self.recorder.as_mut().filter(|r| r.stopping) {
            recorder.transcribe_after_stop = false;
            true
        } else {
            false
        };
        if self.transcription.take().is_some() || pending {
            self.notice = Some("Transcription cancelled.".into());
        }
    }

    pub fn retry_transcription(&mut self, store: &Store) {
        if self.busy() {
            return;
        }
        self.error = None;
        self.notice = None;
        if let Err(error) = self
            .latest_directory(store)
            .and_then(|directory| self.transcribe(directory))
        {
            self.error = Some(error.to_string());
        }
    }

    pub(crate) fn latest_directory(&mut self, store: &Store) -> io::Result<PathBuf> {
        if self.latest.as_ref().is_some_and(|directory| {
            !fs::symlink_metadata(directory.join("audio.caf"))
                .is_ok_and(|metadata| metadata.file_type().is_file())
        }) {
            self.latest = None;
        }
        if self.latest.is_none() {
            self.latest = meeting::latest(store)?;
        }
        self.latest
            .clone()
            .ok_or_else(|| io::Error::other("No postgame recording found for this match."))
    }

    fn transcribe(&mut self, directory: PathBuf) -> io::Result<()> {
        let directory = directory.canonicalize()?;
        let id = nonce();
        let output = directory.join(format!("transcription-{id}.json.tmp"));
        let errors = directory.join(format!("transcription-{id}.stderr"));
        let stdout = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&output)?;
        let stderr = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&errors)?;
        if !self.settings.python.is_file() {
            return Err(io::Error::other(format!(
                "Offline ASR interpreter not found at {}; update Settings. No installation attempted.",
                self.settings.python.display()
            )));
        }
        let child = Command::new("/usr/bin/sandbox-exec")
            .args(["-p", "(version 1) (allow default) (deny network*)"])
            .arg(&self.settings.python)
            .args(["-I", "-B", "-c", include_str!("../native/transcribe.py")])
            .arg(&directory)
            .arg(&self.settings.model_cache)
            .stdin(Stdio::null())
            .stdout(stdout)
            .stderr(stderr)
            .spawn()?;
        self.transcription = Some(TranscriptionJob {
            child,
            directory,
            output,
            errors,
        });
        Ok(())
    }

    pub fn poll(&mut self, store: &Store) {
        if self.preparing.as_ref().is_some_and(|job| job.is_finished()) {
            match self
                .preparing
                .take()
                .unwrap()
                .join()
                .unwrap_or_else(|_| Err(io::Error::other("Audio helper preparation failed")))
            {
                Ok(helper) => {
                    self.helper = Some(helper);
                    self.start_pending(store);
                }
                Err(error) => {
                    self.pending = None;
                    self.error = Some(error.to_string());
                }
            }
        }
        if let Some(recorder) = &mut self.recorder {
            while let Ok(event) = recorder.events.try_recv() {
                match event {
                    Ok(RecorderEvent::Ready) => {
                        if recorder.started.is_none() {
                            recorder.started = Some(Instant::now());
                            if let Err(error) = crate::storage::atomic_write(
                                &recorder.directory.join("recording.started"),
                                b"started\n",
                                false,
                            ) {
                                self.error =
                                    Some(format!("Could not save recording startup: {error}"));
                                let _ = recorder.stop();
                            } else {
                                self.latest = Some(recorder.directory.clone());
                            }
                        }
                    }
                    Ok(RecorderEvent::Stopped { seconds })
                        if seconds.is_finite() && seconds >= 0.0 => {}
                    Ok(_) => self.error = Some("Recorder returned an invalid duration.".into()),
                    Err(error) => {
                        self.error = Some(format!("Recorder communication failed: {error}"));
                        let _ = recorder.stop();
                    }
                }
            }
            match recorder.child.try_wait() {
                Ok(Some(status)) => {
                    let recorder = self.recorder.take().unwrap();
                    if recorder.cancelled {
                        self.notice = Some("Recording cancelled before microphone startup.".into());
                    } else if status.success() && recorder.stopping {
                        if recorder.transcribe_after_stop
                            && let Err(error) = self.transcribe(recorder.directory.clone())
                        {
                            self.error = Some(error.to_string());
                        }
                    } else {
                        self.error = Some(process_error("Recording stopped", &recorder.errors));
                    }
                }
                Err(error) => {
                    self.error = Some(error.to_string());
                    self.recorder.take();
                }
                Ok(None) => {}
            }
        }
        if let Some(job) = &mut self.transcription {
            match job.child.try_wait() {
                Ok(Some(status)) => {
                    let job = self.transcription.take().unwrap();
                    let result = if status.success() {
                        File::open(&job.output)
                            .and_then(|file| {
                                serde_json::from_reader::<_, Transcript>(file)
                                    .map_err(io::Error::other)
                            })
                            .and_then(|transcript| meeting::publish(&job.directory, &transcript))
                    } else {
                        Err(io::Error::other(process_error(
                            "Local transcription failed",
                            &job.errors,
                        )))
                    };
                    match result {
                        Ok(()) => {
                            self.notice = Some("Transcript saved.".into());
                            let _ = fs::remove_file(&job.output);
                            let _ = fs::remove_file(&job.errors);
                        }
                        Err(error) => {
                            self.error = Some(format!(
                                "{error} Audio is retained. Use Retry transcription or Finish without GitHub tabs in the actions menu."
                            ))
                        }
                    }
                }
                Err(error) => {
                    self.error = Some(error.to_string());
                    self.transcription.take();
                }
                Ok(None) => {}
            }
        }
    }
}

impl Drop for Audio {
    fn drop(&mut self) {
        self.recorder.take();
        self.transcription.take();
        if let Some(preparing) = self.preparing.take() {
            let _ = preparing.join();
        }
    }
}

fn nonce() -> String {
    chrono::Local::now().format("%Y%m%dT%H%M%S%.9f").to_string()
}

fn process_error(context: &str, path: &Path) -> String {
    let detail = File::open(path)
        .and_then(|file| {
            use std::io::Read;
            let mut detail = String::new();
            file.take(4096).read_to_string(&mut detail)?;
            Ok(detail)
        })
        .unwrap_or_default();
    format!(
        "{context}: {}",
        if detail.trim().is_empty() {
            "native helper exited unexpectedly"
        } else {
            detail.trim()
        }
    )
}

#[cfg(all(test, unix))]
#[path = "../tests/unit/audio.rs"]
mod tests;
