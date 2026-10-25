use crate::model::MatchType;
use chrono::{DateTime, FixedOffset};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::marker::PhantomData;
use std::net::SocketAddr;
use std::path::Path;

const VERSION: u32 = 1;
const MAX_PACKET_BYTES: usize = 65_535;
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

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RobotPacket {
    pub id: u64,
    pub received_at: DateTime<FixedOffset>,
    pub source: SocketAddr,
    pub packet: Vec<u8>,
}

pub trait Record: Serialize + DeserializeOwned {
    const FORMAT: &'static str;
    fn validate(&self, expected_id: u64) -> io::Result<u64>;
}

impl Record for Packet {
    const FORMAT: &'static str = "robocup-logger-gc";

    fn validate(&self, expected_id: u64) -> io::Result<u64> {
        if self.game == Some(MatchType::Combined) {
            return Err(invalid_data("Capture packet game cannot be Combined"));
        }
        validate_packet(self.id, self.packet.len(), expected_id)
    }
}

impl Record for RobotPacket {
    const FORMAT: &'static str = "robocup-logger-robots";

    fn validate(&self, expected_id: u64) -> io::Result<u64> {
        validate_packet(self.id, self.packet.len(), expected_id)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Header {
    format: String,
    version: u32,
    session_id: String,
}

pub type CaptureLog = PacketLog<Packet>;
pub type RobotCaptureLog = PacketLog<RobotPacket>;

pub struct PacketLog<P> {
    file: File,
    _record: PhantomData<P>,
    next_id: u64,
    committed_len: u64,
    needs_rollback: bool,
}

impl<P: Record> PacketLog<P> {
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
                format: P::FORMAT.into(),
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
            scan::<P>(&mut file, session_id, |_| {})?
        };
        let committed_len = file.seek(SeekFrom::End(0))?;
        Ok(Self {
            file,
            _record: PhantomData,
            next_id,
            committed_len,
            needs_rollback: false,
        })
    }

    pub fn next_id(&self) -> u64 {
        self.next_id
    }

    pub fn append(&mut self, packets: &[P]) -> io::Result<()> {
        self.append_with(packets, |file, bytes| {
            file.write_all(bytes)?;
            file.sync_all()
        })
    }

    fn append_with(
        &mut self,
        packets: &[P],
        write_batch: impl FnOnce(&mut File, &[u8]) -> io::Result<()>,
    ) -> io::Result<()> {
        let mut next_id = self.next_id;
        let mut bytes = Vec::new();
        for packet in packets {
            next_id = packet.validate(next_id)?;
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

    pub fn visit(path: &Path, session_id: &str, collect: impl FnMut(P)) -> io::Result<()> {
        validate_session_id(session_id)?;
        check_regular_file(path)?;
        scan(File::open(path)?, session_id, collect)?;
        Ok(())
    }

    pub fn read(path: &Path, session_id: &str) -> io::Result<Vec<P>> {
        let mut packets = Vec::new();
        Self::visit(path, session_id, |packet| packets.push(packet))?;
        Ok(packets)
    }
}

fn scan<P: Record>(
    file: impl Read,
    session_id: &str,
    mut collect: impl FnMut(P),
) -> io::Result<u64> {
    let mut reader = BufReader::new(file);
    let mut line = Vec::new();
    if !read_line(&mut reader, &mut line)? {
        return Err(invalid_data("Capture header is missing"));
    }
    let header: Header = serde_json::from_slice(&line).map_err(invalid_data)?;
    if header.format != P::FORMAT || header.version != VERSION || header.session_id != session_id {
        return Err(invalid_data(
            "Capture header belongs to another session or unsupported format/version",
        ));
    }
    let mut next_id = 1;
    while read_line(&mut reader, &mut line)? {
        let packet: P = serde_json::from_slice(&line)
            .map_err(|error| invalid_data(format!("Invalid capture packet {next_id}: {error}")))?;
        next_id = packet.validate(next_id)?;
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

fn validate_packet(id: u64, length: usize, expected_id: u64) -> io::Result<u64> {
    if id != expected_id {
        return Err(invalid_data(format!(
            "Capture packet ID {id} is not the expected ID {expected_id}"
        )));
    }
    if length > MAX_PACKET_BYTES {
        return Err(invalid_data("Capture payload exceeds the UDP size limit"));
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

