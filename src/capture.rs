use crate::model::MatchType;
use chrono::{DateTime, FixedOffset};
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::net::SocketAddr;
use std::path::Path;

const FORMAT: &str = "robocup-logger-gc";
const VERSION: u32 = 1;
const MAX_PACKET_BYTES: usize = 65_535;
// JSON represents each payload byte with at most three digits and a comma.
const MAX_RECORD_BYTES: usize = MAX_PACKET_BYTES * 4 + 1024;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Packet {
    pub id: u64,
    pub received_at: DateTime<FixedOffset>,
    pub source: SocketAddr,
    pub true_data: bool,
    pub accepted: bool,
    pub baseline: bool,
    pub packet: Vec<u8>,
    pub game: Option<MatchType>,
    pub active: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Header {
    format: String,
    version: u32,
    session_id: String,
}

pub struct CaptureLog {
    file: File,
    next_id: u64,
    committed_len: u64,
    needs_rollback: bool,
}

impl CaptureLog {
    pub fn open(path: &Path, session_id: &str) -> io::Result<Self> {
        validate_session_id(session_id)?;
        let (mut file, created) = match OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(path)
        {
            Ok(file) => (file, true),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                check_regular_file(path)?;
                (OpenOptions::new().read(true).write(true).open(path)?, false)
            }
            Err(error) => return Err(error),
        };
        let next_id = if created {
            let header = Header {
                format: FORMAT.into(),
                version: VERSION,
                session_id: session_id.into(),
            };
            serde_json::to_writer(&mut file, &header).map_err(invalid_data)?;
            file.write_all(b"\n")?;
            file.sync_all()?;
            File::open(
                path.parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .unwrap_or(Path::new(".")),
            )?
            .sync_all()?;
            1
        } else {
            scan(&mut file, session_id, |_| {})?
        };
        let committed_len = file.seek(SeekFrom::End(0))?;
        Ok(Self {
            file,
            next_id,
            committed_len,
            needs_rollback: false,
        })
    }

    pub fn next_id(&self) -> u64 {
        self.next_id
    }

    pub fn append(&mut self, packets: &[Packet]) -> io::Result<()> {
        self.append_with(packets, |file, bytes| {
            file.write_all(bytes)?;
            file.sync_all()
        })
    }

    fn append_with(
        &mut self,
        packets: &[Packet],
        write_batch: impl FnOnce(&mut File, &[u8]) -> io::Result<()>,
    ) -> io::Result<()> {
        let mut next_id = self.next_id;
        let mut bytes = Vec::new();
        for packet in packets {
            next_id = validate_packet(packet, next_id)?;
            serde_json::to_writer(&mut bytes, packet).map_err(invalid_data)?;
            bytes.push(b'\n');
        }
        if self.needs_rollback {
            self.rollback()?;
        }
        if self.file.metadata()?.len() != self.committed_len {
            return Err(invalid_data(
                "Capture changed outside this logger; refusing to append",
            ));
        }
        if bytes.is_empty() {
            return Ok(());
        }
        self.file.seek(SeekFrom::Start(self.committed_len))?;
        self.needs_rollback = true;
        if let Err(error) = write_batch(&mut self.file, &bytes) {
            if let Err(rollback_error) = self.rollback() {
                return Err(io::Error::new(
                    error.kind(),
                    format!(
                        "Writing capture: {error}; rolling back uncommitted batch: {rollback_error}"
                    ),
                ));
            }
            return Err(error);
        }
        self.committed_len += bytes.len() as u64;
        self.next_id = next_id;
        self.needs_rollback = false;
        Ok(())
    }

    fn rollback(&mut self) -> io::Result<()> {
        self.file.set_len(self.committed_len)?;
        self.file.sync_all()?;
        self.needs_rollback = false;
        Ok(())
    }

    pub fn read(path: &Path, session_id: &str) -> io::Result<Vec<Packet>> {
        validate_session_id(session_id)?;
        check_regular_file(path)?;
        let mut packets = Vec::new();
        scan(File::open(path)?, session_id, |packet| packets.push(packet))?;
        Ok(packets)
    }
}

fn scan(file: impl Read, session_id: &str, mut collect: impl FnMut(Packet)) -> io::Result<u64> {
    let mut reader = BufReader::new(file);
    let mut line = Vec::new();
    if !read_line(&mut reader, &mut line)? {
        return Err(invalid_data("Capture header is missing"));
    }
    let header: Header = serde_json::from_slice(&line).map_err(invalid_data)?;
    if header.format != FORMAT || header.version != VERSION || header.session_id != session_id {
        return Err(invalid_data(
            "Capture header belongs to another session or unsupported format/version",
        ));
    }
    let mut next_id = 1;
    while read_line(&mut reader, &mut line)? {
        let packet: Packet = serde_json::from_slice(&line)
            .map_err(|error| invalid_data(format!("Invalid capture packet {next_id}: {error}")))?;
        next_id = validate_packet(&packet, next_id)?;
        collect(packet);
    }
    Ok(next_id)
}

fn read_line(reader: &mut impl BufRead, line: &mut Vec<u8>) -> io::Result<bool> {
    line.clear();
    let size = reader
        .take((MAX_RECORD_BYTES + 1) as u64)
        .read_until(b'\n', line)?;
    if size == 0 {
        return Ok(false);
    }
    if size > MAX_RECORD_BYTES {
        return Err(invalid_data("Capture record exceeds the size limit"));
    }
    if line.last() != Some(&b'\n') {
        return Err(invalid_data(
            "Capture has a truncated final record; existing bytes were preserved",
        ));
    }
    Ok(true)
}

fn validate_packet(packet: &Packet, expected_id: u64) -> io::Result<u64> {
    if packet.id != expected_id {
        return Err(invalid_data(format!(
            "Capture packet ID {} is not the expected ID {expected_id}",
            packet.id
        )));
    }
    if packet.packet.len() > MAX_PACKET_BYTES {
        return Err(invalid_data("Capture payload exceeds the UDP size limit"));
    }
    if packet.game == Some(MatchType::Combined) {
        return Err(invalid_data("Capture packet game cannot be Combined"));
    }
    expected_id
        .checked_add(1)
        .ok_or_else(|| invalid_data("Capture packet IDs exhausted"))
}

fn validate_session_id(session_id: &str) -> io::Result<()> {
    if session_id.is_empty() || session_id.len() > 256 || session_id.chars().any(char::is_control) {
        return Err(invalid_data("Invalid capture session identifier"));
    }
    Ok(())
}

fn check_regular_file(path: &Path) -> io::Result<()> {
    if !fs::symlink_metadata(path)?.file_type().is_file() {
        return Err(invalid_data(format!(
            "Capture is not a regular file: {}",
            path.display()
        )));
    }
    Ok(())
}

fn invalid_data(error: impl ToString) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn packet(id: u64) -> Packet {
        Packet {
            id,
            received_at: DateTime::parse_from_rfc3339("2026-10-04T13:14:15.123456789+02:00")
                .unwrap(),
            source: "[::1]:3838".parse().unwrap(),
            true_data: true,
            accepted: false,
            baseline: true,
            packet: vec![0, 255, 13, 10, 1],
            game: Some(MatchType::GoldenGoal),
            active: false,
        }
    }

    #[test]
    fn exact_packets_and_receipt_context_survive_reopening() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("capture.jsonl");
        let mut capture = CaptureLog::open(&path, "session").unwrap();
        assert_eq!(capture.next_id(), 1);
        let mut second = packet(2);
        second.accepted = true;
        second.active = true;
        second.baseline = false;
        second.true_data = false;
        second.source = "127.0.0.1:50000".parse().unwrap();
        second.game = Some(MatchType::Official);
        let packets = vec![packet(1), second];
        capture.append(&packets).unwrap();
        assert_eq!(capture.next_id(), 3);
        drop(capture);
        let mut capture = CaptureLog::open(&path, "session").unwrap();
        assert_eq!(capture.next_id(), 3);
        assert_eq!(CaptureLog::read(&path, "session").unwrap(), packets);
        capture.append(&[packet(3)]).unwrap();
        assert_eq!(CaptureLog::read(&path, "session").unwrap().len(), 3);
    }

    #[test]
    fn invalid_batches_do_not_write_any_rows_or_consume_ids() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("capture.jsonl");
        let mut capture = CaptureLog::open(&path, "session").unwrap();
        let before = fs::read(&path).unwrap();
        let mut oversized = packet(2);
        oversized.packet = vec![0; MAX_PACKET_BYTES + 1];
        let mut combined = packet(2);
        combined.game = Some(MatchType::Combined);
        for invalid in [packet(0), packet(1), packet(3), oversized, combined] {
            assert!(capture.append(&[packet(1), invalid]).is_err());
            assert_eq!(fs::read(&path).unwrap(), before);
            assert_eq!(capture.next_id(), 1);
        }
        let mut largest = packet(1);
        largest.packet = vec![255; MAX_PACKET_BYTES];
        capture.append(&[largest.clone()]).unwrap();
        assert_eq!(CaptureLog::read(&path, "session").unwrap(), vec![largest]);
    }

    #[test]
    fn partial_write_and_sync_failure_roll_back_only_the_pending_batch() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("capture.jsonl");
        let mut capture = CaptureLog::open(&path, "session").unwrap();
        capture.append(&[packet(1)]).unwrap();
        let before = fs::read(&path).unwrap();
        for partial in [true, false] {
            assert!(
                capture
                    .append_with(&[packet(2), packet(3)], |file, bytes| {
                        file.write_all(if partial {
                            &bytes[..bytes.len() / 2]
                        } else {
                            bytes
                        })?;
                        Err(io::Error::other("injected write/sync failure"))
                    })
                    .is_err()
            );
            assert_eq!(fs::read(&path).unwrap(), before);
            assert_eq!(capture.next_id(), 2);
        }
        capture.append(&[packet(2), packet(3)]).unwrap();
        assert_eq!(
            CaptureLog::read(&path, "session").unwrap(),
            vec![packet(1), packet(2), packet(3)]
        );
    }

    #[test]
    fn rollback_failure_must_be_retried_before_another_batch() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("capture.jsonl");
        let mut capture = CaptureLog::open(&path, "session").unwrap();
        let before = fs::read(&path).unwrap();
        assert!(
            capture
                .append_with(&[packet(1)], |file, bytes| {
                    file.write_all(&bytes[..10])?;
                    *file = File::open(&path)?;
                    Err(io::Error::other("injected failure"))
                })
                .unwrap_err()
                .to_string()
                .contains("rolling back")
        );
        assert!(capture.append(&[packet(1)]).is_err());
        assert_eq!(capture.next_id(), 1);
        capture.file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        capture.append(&[]).unwrap();
        assert_eq!(fs::read(&path).unwrap(), before);
        capture.append(&[packet(1)]).unwrap();
        assert_eq!(CaptureLog::read(&path, "session").unwrap(), vec![packet(1)]);
    }

    #[test]
    fn foreign_headers_and_damaged_records_are_preserved() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("capture.jsonl");
        let mut capture = CaptureLog::open(&path, "session").unwrap();
        capture.append(&[packet(1)]).unwrap();
        drop(capture);
        let good = fs::read(&path).unwrap();
        let wrong_version = String::from_utf8(good.clone())
            .unwrap()
            .replace("\"version\":1", "\"version\":2")
            .into_bytes();
        let wrong_owner = String::from_utf8(good.clone())
            .unwrap()
            .replace(FORMAT, "another-logger")
            .into_bytes();
        let mut malformed = good.clone();
        malformed.extend(b"{broken}\n");
        let mut missing_id = good.clone();
        missing_id.extend(serde_json::to_vec(&packet(3)).unwrap());
        missing_id.push(b'\n');
        let mut duplicate_id = good.clone();
        duplicate_id.extend(serde_json::to_vec(&packet(1)).unwrap());
        duplicate_id.push(b'\n');
        for damaged in [
            vec![],
            good[..good.len() - 1].to_vec(),
            wrong_version,
            wrong_owner,
            malformed,
            missing_id,
            duplicate_id,
            vec![b'x'; MAX_RECORD_BYTES + 1],
        ] {
            fs::write(&path, &damaged).unwrap();
            assert!(CaptureLog::open(&path, "session").is_err());
            assert!(CaptureLog::read(&path, "session").is_err());
            assert_eq!(fs::read(&path).unwrap(), damaged);
        }
        fs::write(&path, &good).unwrap();
        assert!(CaptureLog::open(&path, "another-session").is_err());
        assert!(CaptureLog::read(&path, "another-session").is_err());
        assert_eq!(fs::read(&path).unwrap(), good);
    }

    #[test]
    fn external_changes_are_not_truncated() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("capture.jsonl");
        let mut capture = CaptureLog::open(&path, "session").unwrap();
        OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"external bytes")
            .unwrap();
        let changed = fs::read(&path).unwrap();
        assert!(capture.append(&[packet(1)]).is_err());
        assert_eq!(fs::read(&path).unwrap(), changed);
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_and_nonfiles_are_refused() {
        use std::os::unix::fs::symlink;
        let directory = tempdir().unwrap();
        let target = directory.path().join("target.jsonl");
        CaptureLog::open(&target, "session").unwrap();
        let before = fs::read(&target).unwrap();
        let link = directory.path().join("link.jsonl");
        symlink(&target, &link).unwrap();
        assert!(CaptureLog::open(&link, "session").is_err());
        assert!(CaptureLog::read(&link, "session").is_err());
        assert!(CaptureLog::open(directory.path(), "session").is_err());
        assert!(CaptureLog::read(directory.path(), "session").is_err());
        assert_eq!(fs::read(&target).unwrap(), before);
    }
}
