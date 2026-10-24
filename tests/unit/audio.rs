use super::*;
use crate::{model::MatchType, test_support::session};

#[test]
fn stopping_and_cancelling_recording_keeps_files_without_launching_transcription() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::create(dir.path(), &session(MatchType::Official)).unwrap();
    let path = dir.path().join("audio.caf");
    fs::write(&path, b"retained audio").unwrap();
    let mut child = Command::new("cat")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    let input = child.stdin.take();
    let (_, events) = mpsc::channel();
    let mut audio = Audio::default();
    assert!(!audio.busy());
    audio.recorder = Some(Recorder {
        child,
        input,
        events,
        directory: dir.path().into(),
        errors: dir.path().join("errors"),
        started: Some(Instant::now()),
        stopping: false,
        cancelled: false,
        transcribe_after_stop: true,
    });
    audio.mark(None);
    audio.finish();
    audio.cancel_transcription();
    let mut line = String::new();
    output.read_line(&mut line).unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&line).unwrap()["command"],
        "mark"
    );
    line.clear();
    output.read_line(&mut line).unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&line).unwrap()["command"],
        "stop"
    );
    audio.recorder.as_mut().unwrap().child.wait().unwrap();
    audio.poll(&store);
    assert!(!audio.busy());
    assert!(audio.error.is_none());
    assert_eq!(fs::read(path).unwrap(), b"retained audio");
}

#[test]
fn transcription_cancellation_reaps_the_child_and_preserves_partial_output() {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("partial.json");
    fs::write(&output, "partial").unwrap();
    let mut child = Command::new("cat")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut audio = Audio::default();
    audio.transcription = Some(TranscriptionJob {
        child,
        directory: dir.path().into(),
        output: output.clone(),
        errors: dir.path().join("errors"),
    });
    audio.cancel_transcription();
    assert!(!audio.busy());
    assert!(input.write_all(b"must be reaped").is_err());
    assert_eq!(fs::read_to_string(output).unwrap(), "partial");
}
