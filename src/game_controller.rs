use std::{
    io,
    net::{SocketAddr, UdpSocket},
    time::{Duration, Instant},
};

use chrono::{DateTime, FixedOffset, Local};

use crate::capture::Packet;

pub const HULKS_TEAM_NUMBER: u8 = 24;
const PACKET_SIZE: usize = 158;
const STALE_AFTER: Duration = Duration::from_secs(5);
const TRUE_DATA_RETRY_AFTER: Duration = Duration::from_secs(1);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub packet_number: u8,
    pub game_phase: u8,
    pub state: u8,
    pub set_play: u8,
    pub first_half: bool,
    pub stopped: bool,
    pub kicking_team: u8,
    pub seconds_remaining: i16,
    pub secondary_time: i16,
    pub hulks_score: u8,
    pub hulks_message_budget: u16,
    pub hulks_penalties: [u8; 20],
    pub opponent_score: u8,
    pub opponent_number: u8,
}

impl Snapshot {
    pub fn clock(&self) -> String {
        format_clock(self.seconds_remaining)
    }

    pub fn summary(&self) -> String {
        let phase = match self.game_phase {
            0 => "Normal",
            1 => "Penalty shootout",
            2 => "Extra time",
            3 => "Timeout",
            _ => "Unknown phase",
        };
        let state = match self.state {
            0 => "Initial",
            1 => "Ready",
            2 => "Set",
            3 => "Playing",
            4 => "Finished",
            _ => "Unknown state",
        };
        let set_play = match self.set_play {
            0 => "None",
            1 => "Direct free kick",
            2 => "Indirect free kick",
            3 => "Penalty kick",
            4 => "Throw-in",
            5 => "Goal kick",
            6 => "Corner kick",
            _ => "Unknown set play",
        };
        let kicking_team = if self.kicking_team == 255 {
            "none".to_owned()
        } else {
            self.kicking_team.to_string()
        };
        format!(
            "{phase} / {state} / {} half; set play: {set_play}; kicking team: {kicking_team}; stopped: {}; clock {}; secondary {}",
            if self.first_half { "first" } else { "second" },
            if self.stopped { "yes" } else { "no" },
            self.clock(),
            format_clock(self.secondary_time),
        )
    }

    fn same_game_state(&self, other: &Self) -> bool {
        self.same_match_state(other) && self.hulks_penalties == other.hulks_penalties
    }

    pub(crate) fn same_match_state(&self, other: &Self) -> bool {
        self.game_phase == other.game_phase
            && self.state == other.state
            && self.set_play == other.set_play
            && self.first_half == other.first_half
            && self.stopped == other.stopped
            && self.kicking_team == other.kicking_team
            && self.hulks_score == other.hulks_score
            && self.opponent_score == other.opponent_score
            && self.opponent_number == other.opponent_number
    }
}

pub(crate) fn format_clock(seconds: i16) -> String {
    let magnitude = i32::from(seconds).abs();
    format!(
        "{}{:02}:{:02}",
        if seconds < 0 { "-" } else { "" },
        magnitude / 60,
        magnitude % 60,
    )
}

// v20 layout: https://github.com/RoboCup-SPL/GameController/blob/7194909358cc6f93fecac93d3a08cbe6c17bd888/src/data/GameControlData.java
fn parse(packet: &[u8]) -> Result<Option<Snapshot>, String> {
    parse_feed(packet, false)
}

pub(crate) fn decode_capture(packet: &[u8], true_data: bool) -> Result<Option<Snapshot>, String> {
    parse_feed(packet, true_data)
}

fn parse_feed(packet: &[u8], true_data: bool) -> Result<Option<Snapshot>, String> {
    let (selected, ignored) = if true_data {
        (b"RGTD", b"RGme")
    } else {
        (b"RGme", b"RGTD")
    };
    if packet.get(..4) == Some(ignored) {
        return Ok(None);
    }
    if packet.get(..4) != Some(selected) {
        return Err("Ignored GameController packet: invalid header".to_owned());
    }
    let Some(&version) = packet.get(4) else {
        return Err("Ignored GameController packet: missing version".to_owned());
    };
    if version != 20 {
        return Err(format!(
            "Unsupported GameController version {version}; only v20 is supported"
        ));
    }
    if packet.len() != PACKET_SIZE {
        return Err(format!(
            "Ignored GameController v20 packet: expected {PACKET_SIZE} bytes, got {}",
            packet.len()
        ));
    }
    if packet[6] > 20
        || packet[7] > 2
        || packet[8] > 1
        || packet[9] > 3
        || packet[10] > 4
        || packet[11] > 6
        || packet[12] > 1
    {
        return Err("Ignored GameController v20 packet: invalid state or enum value".to_owned());
    }

    let (hulks, opponent) = match (packet[18], packet[88]) {
        (HULKS_TEAM_NUMBER, HULKS_TEAM_NUMBER) => {
            return Err("Ignored GameController packet: duplicate HULKs team ID".to_owned());
        }
        (HULKS_TEAM_NUMBER, _) => (18, 88),
        (_, HULKS_TEAM_NUMBER) => (88, 18),
        _ => return Ok(None),
    };
    if packet[13] != 255 && packet[13] != packet[18] && packet[13] != packet[88] {
        return Err("Ignored GameController packet: invalid kicking team ID".to_owned());
    }

    Ok(Some(Snapshot {
        packet_number: packet[5],
        game_phase: packet[9],
        state: packet[10],
        set_play: packet[11],
        first_half: packet[12] == 1,
        stopped: packet[8] == 1,
        kicking_team: packet[13],
        seconds_remaining: i16::from_le_bytes([packet[14], packet[15]]),
        secondary_time: i16::from_le_bytes([packet[16], packet[17]]),
        hulks_score: packet[hulks + 4],
        hulks_message_budget: u16::from_le_bytes([packet[hulks + 8], packet[hulks + 9]]),
        hulks_penalties: std::array::from_fn(|player| packet[hulks + 10 + 3 * player]),
        opponent_score: packet[opponent + 4],
        opponent_number: packet[opponent],
    }))
}

#[derive(Debug)]
pub struct Update {
    pub snapshot: Snapshot,
    pub previous: Option<Snapshot>,
    pub received_at: DateTime<FixedOffset>,
    pub capture_id: Option<u64>,
}

pub struct Receiver {
    socket: UdpSocket,
    capture_enabled: bool,
    next_capture_id: u64,
    pub captures: Vec<Packet>,
    opponent_number: Option<u8>,
    true_source: Option<SocketAddr>,
    retired_true_source: Option<SocketAddr>,
    last_true_receipt: Option<Instant>,
    last_true_data_request: Option<Instant>,
    #[cfg(test)]
    request_destination: Option<SocketAddr>,
    pub true_data: bool,
    auto_true_data: bool,
    pub allow_opponent_change: bool,
    pub ignored_opponent: Option<u8>,
    pub latest: Option<Snapshot>,
    pub received_at: Option<Instant>,
    pub source: Option<SocketAddr>,
    pub notice: Option<String>,
}

impl Receiver {
    pub fn bind() -> io::Result<Self> {
        let socket = UdpSocket::bind("0.0.0.0:3838")?;
        socket.set_nonblocking(true)?;
        Ok(Self {
            socket,
            capture_enabled: false,
            next_capture_id: 0,
            captures: Vec::new(),
            opponent_number: None,
            true_source: None,
            retired_true_source: None,
            last_true_receipt: None,
            last_true_data_request: None,
            #[cfg(test)]
            request_destination: None,
            true_data: false,
            auto_true_data: true,
            allow_opponent_change: false,
            ignored_opponent: None,
            latest: None,
            received_at: None,
            source: None,
            notice: None,
        })
    }

    #[cfg(test)]
    pub fn for_test() -> Self {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        socket.set_nonblocking(true).unwrap();
        Self {
            socket,
            capture_enabled: false,
            next_capture_id: 0,
            captures: Vec::new(),
            opponent_number: None,
            true_source: None,
            retired_true_source: None,
            last_true_receipt: None,
            last_true_data_request: None,
            request_destination: None,
            true_data: false,
            // Existing fixtures exercise broadcast mode; discovery tests opt in.
            auto_true_data: false,
            allow_opponent_change: false,
            ignored_opponent: None,
            latest: None,
            received_at: None,
            source: None,
            notice: None,
        }
    }

    pub fn enable_capture(&mut self, next_id: u64) {
        self.next_capture_id = next_id;
        self.capture_enabled = true;
    }

    pub fn next_capture_id(&self) -> u64 {
        self.next_capture_id
    }

    pub fn set_true_data(&mut self, enabled: bool) -> io::Result<()> {
        if enabled && self.source.is_none() {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "Wait for a GameController broadcast before requesting true data",
            ));
        }
        self.auto_true_data = false;
        if self.true_data != enabled {
            self.true_data = enabled;
            self.true_source = None;
            self.retired_true_source = None;
            self.last_true_receipt = None;
            self.last_true_data_request = None;
            self.rebaseline();
            self.notice = None;
        }
        Ok(())
    }

    pub fn poll(&mut self) -> io::Result<Vec<Update>> {
        let mut updates = Vec::new();
        // Preserve the complete UDP payload, including malformed/overlong frames.
        let mut buffer = [0; 65536];
        for _ in 0..64 {
            let (length, source) = match self.socket.recv_from(&mut buffer) {
                Ok(packet) => packet,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    self.notice = Some(format!("GameController receive failed: {error}"));
                    // Preserve updates already consumed before an error.
                    if updates.is_empty() {
                        return Err(error);
                    }
                    break;
                }
            };
            let received_at = Local::now().fixed_offset();
            let now = Instant::now();
            let true_data = self.true_data;
            let parsed = if true_data {
                parse_feed(&buffer[..length], true)
            } else {
                parse(&buffer[..length])
            };
            let (accepted, mut update) = match parsed {
                Ok(Some(snapshot)) => {
                    let (accepted, update) = self.accept_packet(snapshot, source, now, received_at);
                    if update.is_some() && self.auto_true_data {
                        // Discover the controller, not its delayed match state/opponent.
                        self.set_true_data(true)?;
                        self.opponent_number = None;
                        (false, None)
                    } else {
                        (accepted, update)
                    }
                }
                Ok(None) => (false, None),
                Err(notice) => {
                    let (header, pinned) = if self.true_data {
                        (b"RGTD", self.true_source)
                    } else {
                        (b"RGme", self.source)
                    };
                    if buffer[..length].get(..4) == Some(header) && pinned == Some(source) {
                        // Missing state from the selected stream breaks statistical coverage.
                        self.rebaseline();
                    }
                    self.notice = Some(notice);
                    (false, None)
                }
            };
            if self.capture_enabled
                && self.source.is_some_and(|pinned| pinned.ip() == source.ip())
                && matches!(buffer[..length].get(..4), Some(b"RGme" | b"RGTD"))
            {
                let id = self.next_capture_id;
                self.next_capture_id += 1;
                self.captures.push(Packet {
                    id,
                    received_at,
                    source,
                    true_data,
                    accepted,
                    baseline: update
                        .as_ref()
                        .is_some_and(|update| update.previous.is_none()),
                    packet: buffer[..length].to_vec(),
                    game: None,
                    active: false,
                });
                if let Some(update) = &mut update {
                    update.capture_id = Some(id);
                }
            }
            if let Some(update) = update {
                updates.push(update);
            }
        }
        if let Err(error) = self.request_true_data(Instant::now()) {
            self.notice = Some(format!("GameController true-data request failed: {error}"));
        }
        Ok(updates)
    }

    fn request_true_data(&mut self, now: Instant) -> io::Result<()> {
        // HSL recreates the monitor sender (port and sequence) on every request.
        // Like GameControlDataReceiver.java, only retry after a second without true data.
        if !self.true_data
            || self
                .last_true_receipt
                .is_some_and(|last| now.saturating_duration_since(last) < TRUE_DATA_RETRY_AFTER)
            || self
                .last_true_data_request
                .is_some_and(|last| now.saturating_duration_since(last) < TRUE_DATA_RETRY_AFTER)
        {
            return Ok(());
        }
        self.last_true_data_request = Some(now);
        #[cfg(not(test))]
        let destination = SocketAddr::new(
            self.source
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotConnected, "No controller source"))?
                .ip(),
            3636,
        );
        #[cfg(test)]
        let destination = self.request_destination.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotConnected,
                "No loopback request destination",
            )
        })?;
        #[cfg(test)]
        assert!(
            destination.ip().is_loopback() && ![0, 3636, 3838].contains(&destination.port()),
            "Tests must use an ephemeral loopback request destination"
        );
        self.socket.send_to(b"RGTr\0", destination)?;
        // Retire the old port before accepting the recreated sender's new baseline.
        // Keep it retired across unanswered retries: in-flight packets cannot repin it.
        if let Some(source) = self.true_source.take() {
            self.retired_true_source = Some(source);
        }
        Ok(())
    }

    pub fn is_stale(&self) -> bool {
        self.is_stale_at(Instant::now())
    }

    fn is_stale_at(&self, now: Instant) -> bool {
        self.received_at
            .is_none_or(|last| now.saturating_duration_since(last) > STALE_AFTER)
    }

    pub fn rebaseline(&mut self) {
        self.latest = None;
        self.received_at = None;
    }

    #[cfg(test)]
    fn accept(
        &mut self,
        snapshot: Snapshot,
        source: SocketAddr,
        now: Instant,
        received_at: DateTime<FixedOffset>,
    ) -> Option<Update> {
        self.accept_packet(snapshot, source, now, received_at).1
    }

    // Acceptance includes clock-only frames, which deliberately produce no Update.
    fn accept_packet(
        &mut self,
        snapshot: Snapshot,
        source: SocketAddr,
        now: Instant,
        received_at: DateTime<FixedOffset>,
    ) -> (bool, Option<Update>) {
        if self.ignored_opponent == Some(snapshot.opponent_number)
            || (self.true_data && self.retired_true_source == Some(source))
        {
            return (false, None);
        }
        let source_matches = if self.true_data {
            self.source.is_some_and(|pinned| pinned.ip() == source.ip())
                && self.true_source.is_none_or(|pinned| pinned == source)
        } else {
            self.source.is_none_or(|pinned| pinned == source)
        };
        if !source_matches {
            self.notice = Some(format!(
                "Ignored GameController source {source}; toggle GC off/on to switch controller"
            ));
            return (false, None);
        }
        let opponent_changed = self
            .opponent_number
            .is_some_and(|pinned| pinned != snapshot.opponent_number);
        if opponent_changed && !self.allow_opponent_change {
            self.notice = Some(format!(
                "Ignored GameController opponent {}; toggle GC off/on to switch opponent",
                snapshot.opponent_number
            ));
            return (false, None);
        }
        // A new game can reset its sequence counter even without a stale interval.
        let mut previous = if opponent_changed
            || self.is_stale_at(now)
            || (self.true_data && self.true_source.is_none())
        {
            None
        } else {
            self.latest.clone()
        };
        if let Some(last) = &previous {
            let delta = snapshot.packet_number.wrapping_sub(last.packet_number);
            if !(1..128).contains(&delta) {
                return (false, None);
            }
            if delta != 1 {
                // Lost packets may contain complete penalty or restart episodes.
                previous = None;
            }
        }
        let changed = previous
            .as_ref()
            .is_none_or(|last| !snapshot.same_game_state(last));
        if self.true_data {
            self.true_source = Some(source);
            self.last_true_receipt = Some(now);
        } else {
            self.source = Some(source);
        }
        // A reopened receiver may attach directly to the new game. Pin that first
        // opponent for the rest of this batch as well, until the app imports it.
        if opponent_changed || self.opponent_number.is_none() {
            self.allow_opponent_change = false;
        }
        self.opponent_number = Some(snapshot.opponent_number);
        self.received_at = Some(now);
        self.latest = Some(snapshot.clone());
        self.notice = None;
        (
            true,
            changed.then_some(Update {
                snapshot,
                previous,
                received_at,
                capture_id: None,
            }),
        )
    }
}
