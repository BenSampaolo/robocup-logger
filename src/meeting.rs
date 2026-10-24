use std::{
    fmt::Write as _,
    fs::{self, File},
    io::{self, BufRead, BufReader},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use chrono::{DateTime, FixedOffset, Local};
use serde::{Deserialize, Serialize};

use crate::{
    game_controller::format_clock,
    model::{Entry, EventKind, MatchType, Metadata, Session},
    storage::{Store, atomic_write},
};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct NoteSnapshot {
    pub entry_index: usize,
    pub timestamp: DateTime<FixedOffset>,
    pub gc_seconds_remaining: Option<i16>,
    pub kind: EventKind,
    pub game: Option<MatchType>,
    pub text: String,
}

impl NoteSnapshot {
    pub(crate) fn from_entry(index: usize, entry: &Entry, session: &Session) -> Self {
        Self {
            entry_index: index,
            timestamp: entry.timestamp,
            gc_seconds_remaining: entry.gc_seconds_remaining,
            kind: entry.kind,
            game: entry.game.or(session.match_type),
            text: entry.text.clone(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct MeetingInfo {
    pub started_at: DateTime<FixedOffset>,
    pub locale: String,
    pub metadata: Metadata,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Marker {
    pub seconds: f64,
    pub note: Option<NoteSnapshot>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Segment {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Transcript {
    pub duration_seconds: f64,
    pub segments: Vec<Segment>,
}

pub(crate) fn create(store: &Store, session: &Session) -> io::Result<PathBuf> {
    session.metadata.validate().map_err(invalid_data)?;
    let root = store.session_path().with_extension("postgame");
    match create_private_directory(&root) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    require_directory(&root)?;

    let info = MeetingInfo {
        started_at: Local::now().fixed_offset(),
        locale: "de-DE".into(),
        metadata: session.metadata.clone(),
    };
    static NEXT_MEETING: AtomicU64 = AtomicU64::new(0);
    let directory = loop {
        let id = NEXT_MEETING.fetch_add(1, Ordering::Relaxed);
        let path = root.join(format!(
            "meeting-{}-{}-{id}",
            info.started_at.format("%Y%m%dT%H%M%S%.9f%z"),
            std::process::id(),
        ));
        match create_private_directory(&path) {
            Ok(()) => break path,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    };
    let json = serde_json::to_vec_pretty(&info).map_err(invalid_data)?;
    atomic_write(&directory.join("metadata.json"), &json, false)?;
    Ok(directory)
}

pub(crate) fn latest(store: &Store) -> io::Result<Option<PathBuf>> {
    let root = store.session_path().with_extension("postgame");
    match fs::symlink_metadata(&root) {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => return Err(invalid_data("Meeting root is not a regular directory")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    }
    let mut latest: Option<(DateTime<FixedOffset>, PathBuf)> = None;
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let path = entry.path();
        let info = match read_info(&path).and_then(|info| {
            open_regular(&path.join("audio.caf"))?;
            open_regular(&path.join("recording.started"))?;
            Ok(info)
        }) {
            Ok(info) => info,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::InvalidData | io::ErrorKind::NotFound
                ) =>
            {
                continue;
            }
            Err(error) => return Err(error),
        };
        let candidate = (info.started_at, path);
        if latest.as_ref().is_none_or(|current| candidate > *current) {
            latest = Some(candidate);
        }
    }
    Ok(latest.map(|(_, path)| path))
}

pub(crate) fn publish(directory: &Path, transcript: &Transcript) -> io::Result<()> {
    require_directory(directory)?;
    let info = read_info(directory)?;
    let markers: Vec<Marker> = BufReader::new(open_regular(&directory.join("timeline.jsonl"))?)
        .lines()
        .map(|line| serde_json::from_str(&line?).map_err(invalid_data))
        .collect::<io::Result<_>>()?;
    validate_timeline(&markers, transcript)?;
    let json = serde_json::to_vec_pretty(transcript).map_err(invalid_data)?;
    let markdown = render(&info, &markers, transcript);
    let json_path = directory.join("transcript.json");
    let markdown_path = directory.join("transcript.md");
    // Check both destinations before replacing either, including on a failed-save retry.
    let replace_json = regular_file_exists(&json_path)?;
    let replace_markdown = regular_file_exists(&markdown_path)?;
    atomic_write(&json_path, &json, replace_json)?;
    atomic_write(&markdown_path, markdown.as_bytes(), replace_markdown)
}

pub(crate) struct TranscriptData {
    pub info: MeetingInfo,
    pub markers: Vec<Marker>,
    pub transcript: Transcript,
}

pub(crate) fn read_transcript_data(directory: &Path) -> io::Result<TranscriptData> {
    require_directory(directory)?;
    let info = read_info(directory)?;
    let markers = BufReader::new(open_regular(&directory.join("timeline.jsonl"))?)
        .lines()
        .map(|line| serde_json::from_str(&line?).map_err(invalid_data))
        .collect::<io::Result<Vec<Marker>>>()?;
    let transcript = serde_json::from_reader(open_regular(&directory.join("transcript.json"))?)
        .map_err(invalid_data)?;
    validate_timeline(&markers, &transcript)?;
    Ok(TranscriptData {
        info,
        markers,
        transcript,
    })
}

fn create_private_directory(path: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)
}

fn invalid_data(error: impl ToString) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

fn require_directory(path: &Path) -> io::Result<()> {
    if !fs::symlink_metadata(path)?.is_dir() {
        return Err(invalid_data(format!(
            "Not a regular directory: {}",
            path.display()
        )));
    }
    Ok(())
}

fn regular_file_exists(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() => Ok(true),
        Ok(_) => Err(invalid_data(format!(
            "Not a regular file: {}",
            path.display()
        ))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn open_regular(path: &Path) -> io::Result<File> {
    if !regular_file_exists(path)? {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("Missing meeting file: {}", path.display()),
        ));
    }
    File::open(path)
}

fn read_info(directory: &Path) -> io::Result<MeetingInfo> {
    let info: MeetingInfo =
        serde_json::from_reader(open_regular(&directory.join("metadata.json"))?)
            .map_err(invalid_data)?;
    if info.locale != "de-DE" {
        return Err(invalid_data("Unrecognized meeting transcription locale"));
    }
    info.metadata.validate().map_err(invalid_data)?;
    Ok(info)
}

fn validate_timeline(markers: &[Marker], transcript: &Transcript) -> io::Result<()> {
    let duration = transcript.duration_seconds;
    if !duration.is_finite() || duration < 0.0 {
        return Err(invalid_data("Invalid recording duration"));
    }
    let mut previous = 0.0;
    for marker in markers {
        if !marker.seconds.is_finite() || marker.seconds < previous || marker.seconds > duration {
            return Err(invalid_data(
                "Timeline markers must be ordered within the recording",
            ));
        }
        previous = marker.seconds;
    }
    previous = 0.0;
    for segment in &transcript.segments {
        if !segment.start.is_finite()
            || !segment.end.is_finite()
            || segment.start < previous
            || segment.end < segment.start
            || segment.end > duration
        {
            return Err(invalid_data(
                "Speech segments must be ordered within the recording",
            ));
        }
        previous = segment.start;
    }
    Ok(())
}

fn render(info: &MeetingInfo, markers: &[Marker], transcript: &Transcript) -> String {
    let mut output = format!(
        "# Postgame meeting transcript\n\n{} — {} vs {}\n\nMatch: {} ({})\nMeeting started: {}\nLanguage: {}\nRecorded audio: 0.000–{:.3} s\n\nNote boundaries are approximate at microphone-buffer granularity (typically 100–400 ms). Speech recognition and word times may be inaccurate; consult audio.caf when uncertain.\n\n",
        info.metadata.event,
        info.metadata.home,
        info.metadata.away,
        info.metadata.label,
        info.metadata.date,
        info.started_at.to_rfc3339(),
        info.locale,
        transcript.duration_seconds,
    );
    let mut windows = Vec::new();
    if markers.first().is_none_or(|marker| marker.seconds > 0.0) {
        windows.push((
            0.0,
            markers
                .first()
                .map_or(transcript.duration_seconds, |marker| marker.seconds),
            None,
        ));
    }
    for (index, marker) in markers.iter().enumerate() {
        windows.push((
            marker.seconds,
            markers
                .get(index + 1)
                .map_or(transcript.duration_seconds, |next| next.seconds),
            marker.note.as_ref(),
        ));
    }
    // Group revisits by the immutable source entry, retaining each visit's own snapshot.
    let mut groups: Vec<(Option<usize>, Vec<usize>)> = Vec::new();
    for (index, &(_, _, note)) in windows.iter().enumerate() {
        let source = note.map(|note| note.entry_index);
        if let Some((_, visits)) = groups.iter_mut().find(|(key, _)| *key == source) {
            visits.push(index);
        } else {
            groups.push((source, vec![index]));
        }
    }
    output.push_str("Discussion is grouped by note/task in first-presentation order. Each visit keeps its original audio interval; the full audio is unchanged.\n\n");
    for (source, visits) in groups {
        match source {
            Some(index) => {
                let _ = writeln!(output, "## Note/task (source index {index})\n");
            }
            None => output.push_str("## Discussion without a presented note\n\n"),
        }
        for (visit, index) in visits.into_iter().enumerate() {
            let (start, end, note) = windows[index];
            let _ = writeln!(
                output,
                "### Audio {start:.3}–{end:.3} s · Visit {}",
                visit + 1
            );
            if let Some(note) = note {
                let gc = note
                    .gc_seconds_remaining
                    .map(format_clock)
                    .unwrap_or_else(|| "unavailable".into());
                let game = match note.game {
                    Some(MatchType::Official) => "Official match",
                    Some(MatchType::GoldenGoal) => "Golden Goal",
                    Some(MatchType::Combined) => "Golden Goal + official",
                    None => "unspecified",
                };
                let _ = writeln!(
                    output,
                    "\nPresented note (source index {}; retained snapshot)\nOriginal wall-clock: {}\nGC: {gc}\nGame: {game}\nEvent: {}\n\n#### Original note\n\n{}\n",
                    note.entry_index,
                    note.timestamp.to_rfc3339(),
                    note.kind.label(),
                    note.text,
                );
            } else {
                output.push_str("\nNo note presented.\n\n");
            }
            output.push_str("#### Speech\n\n");
            let mut speech = Vec::new();
            for segment in &transcript.segments {
                // Point segments belong to the new window at a switch, or the final window at EOF.
                let overlaps = if segment.start == segment.end {
                    segment.start >= start
                        && (segment.start < end
                            || (index + 1 == windows.len() && segment.start == end))
                } else {
                    segment.start < end && segment.end > start && start < end
                };
                if !overlaps {
                    continue;
                }
                speech.push(segment);
            }
            if let Some(first) = speech.first() {
                let last_end = speech
                    .iter()
                    .map(|segment| segment.end)
                    .fold(first.end, f64::max);
                let overlap = if first.start < start || last_end > end {
                    " (overlaps presentation windows; repeated)"
                } else {
                    ""
                };
                let text = speech
                    .iter()
                    .map(|segment| segment.text.trim())
                    .collect::<Vec<_>>()
                    .join(" ");
                let _ = writeln!(
                    output,
                    "[{:.3}–{last_end:.3} s]{overlap} {text}\n",
                    first.start
                );
            } else {
                output.push_str("No recognized speech segments in this window.\n\n");
            }
        }
    }
    output
}

#[cfg(test)]
#[path = "../tests/unit/meeting.rs"]
mod tests;
