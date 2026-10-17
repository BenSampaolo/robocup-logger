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

#[cfg(test)]
mod tests {
    use super::*;

    // Synthetic wire fixture, independently laid out from the upstream v20 fields; not a capture.
    fn packet(sequence: u8) -> [u8; 158] {
        let mut bytes = [0; 158];
        bytes[..18].copy_from_slice(&[
            b'R', b'G', b'm', b'e', 20, sequence, 7, 0, 1, 2, 3, 6, 1, 24, 0xc7, 0xfe, 0x2d, 0x01,
        ]);
        bytes[18] = 24;
        bytes[22] = 3;
        bytes[26..28].copy_from_slice(&1025_u16.to_le_bytes());
        bytes[88] = 5;
        bytes[92] = 2;
        bytes
    }

    fn snapshot(sequence: u8) -> Snapshot {
        parse(&packet(sequence)).unwrap().unwrap()
    }

    fn true_packet(sequence: u8) -> [u8; 158] {
        let mut bytes = packet(sequence);
        bytes[..4].copy_from_slice(b"RGTD");
        bytes
    }

    fn request_endpoint(receiver: &mut Receiver) -> UdpSocket {
        let endpoint = UdpSocket::bind("127.0.0.1:0").unwrap();
        endpoint
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        receiver.request_destination = Some(endpoint.local_addr().unwrap());
        endpoint
    }

    fn assert_request(endpoint: &UdpSocket, receiver: &Receiver) {
        let mut bytes = [0; 16];
        let (length, source) = endpoint.recv_from(&mut bytes).unwrap();
        assert_eq!(&bytes[..length], b"RGTr\0");
        assert_eq!(source, receiver.socket.local_addr().unwrap());
    }

    fn assert_no_datagram(endpoint: &UdpSocket) {
        endpoint.set_nonblocking(true).unwrap();
        assert_eq!(
            endpoint.recv_from(&mut [0; 16]).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        endpoint.set_nonblocking(false).unwrap();
    }

    fn receiver() -> Receiver {
        Receiver::for_test()
    }

    fn source() -> SocketAddr {
        "127.0.0.1:50000".parse().unwrap()
    }

    fn wall_time() -> DateTime<FixedOffset> {
        DateTime::parse_from_rfc3339("2026-10-03T12:30:00+02:00").unwrap()
    }

    #[test]
    fn parses_v20_offsets_and_signed_clocks() {
        let data = snapshot(254);
        assert_eq!(
            data,
            Snapshot {
                packet_number: 254,
                game_phase: 2,
                state: 3,
                set_play: 6,
                first_half: true,
                stopped: true,
                kicking_team: 24,
                seconds_remaining: -313,
                secondary_time: 301,
                hulks_score: 3,
                hulks_message_budget: 1025,
                hulks_penalties: [0; 20],
                opponent_score: 2,
                opponent_number: 5,
            }
        );
        assert_eq!(data.clock(), "-05:13");
        assert!(data.summary().contains("Corner kick"));
        assert!(data.summary().contains("secondary 05:01"));
        assert_eq!(format_clock(i16::MIN), "-546:08");
        assert_eq!(format_clock(0), "00:00");
    }

    #[test]
    fn rejects_bad_packets_and_ignores_true_data() {
        let bytes = packet(0);
        for length in 0..bytes.len() {
            assert!(parse(&bytes[..length]).is_err(), "length {length}");
        }
        let mut overlong = bytes.to_vec();
        overlong.push(0);
        assert!(parse(&overlong).is_err());
        let mut bad = bytes;
        bad[0] = b'x';
        assert!(parse(&bad).is_err());
        let mut old = bytes;
        old[4] = 18;
        assert!(parse(&old[..118]).unwrap_err().contains("version 18"));
        let mut true_data = bytes;
        true_data[..4].copy_from_slice(b"RGTD");
        assert_eq!(parse(&true_data).unwrap(), None);
        for (offset, value) in [
            (6, 21),
            (7, 3),
            (8, 2),
            (9, 4),
            (10, 5),
            (11, 7),
            (12, 2),
            (13, 6),
        ] {
            let mut bad = bytes;
            bad[offset] = value;
            assert!(parse(&bad).is_err(), "offset {offset}");
        }
    }

    #[test]
    fn true_feed_uses_the_same_v20_body_and_rejects_bad_packets() {
        let mut bytes = packet(254);
        bytes[..4].copy_from_slice(b"RGTD");
        assert_eq!(parse_feed(&bytes, true).unwrap(), Some(snapshot(254)));
        assert_eq!(parse_feed(&packet(254), true).unwrap(), None);
        assert_eq!(parse(&bytes).unwrap(), None);
        for length in 0..bytes.len() {
            assert!(
                parse_feed(&bytes[..length], true).is_err(),
                "length {length}"
            );
        }
        let mut overlong = bytes.to_vec();
        overlong.push(0);
        assert!(parse_feed(&overlong, true).is_err());
        bytes[4] = 18;
        assert!(
            parse_feed(&bytes[..118], true)
                .unwrap_err()
                .contains("version 18")
        );
        bytes[4] = 20;
        bytes[10] = 5;
        assert!(parse_feed(&bytes, true).is_err());
    }

    #[test]
    fn filters_teams_and_normalizes_side_swaps() {
        let mut bytes = packet(2);
        bytes[18] = 7;
        assert_eq!(parse(&bytes).unwrap(), None);
        bytes[18] = 24;
        bytes[88] = 24;
        assert!(parse(&bytes).is_err());
        let bytes = packet(2);
        let mut swapped = bytes;
        swapped[18..88].copy_from_slice(&bytes[88..158]);
        swapped[88..158].copy_from_slice(&bytes[18..88]);
        assert_eq!(parse(&swapped), parse(&bytes));
    }

    #[test]
    fn parses_all_penalty_codes_in_both_team_orientations() {
        let mut bytes = packet(2);
        let expected = std::array::from_fn(|player| player as u8);
        for (player, code) in expected.into_iter().enumerate() {
            bytes[28 + 3 * player..31 + 3 * player].copy_from_slice(&[code, 60, 3]);
            bytes[98 + 3 * player..101 + 3 * player].copy_from_slice(&[99, 50, 2]);
        }
        assert_eq!(parse(&bytes).unwrap().unwrap().hulks_penalties, expected);
        let mut swapped = bytes;
        swapped[18..88].copy_from_slice(&bytes[88..158]);
        swapped[88..158].copy_from_slice(&bytes[18..88]);
        assert_eq!(parse(&swapped), parse(&bytes));
    }

    #[test]
    fn penalty_code_changes_emit_once_but_countdowns_and_cautions_are_quiet() {
        let mut receiver = receiver();
        let now = Instant::now();
        let mut bytes = packet(10);
        let initial = parse(&bytes).unwrap().unwrap();
        receiver
            .accept(initial.clone(), source(), now, wall_time())
            .unwrap();
        bytes[5] += 1;
        bytes[29] = 30;
        bytes[30] = 2;
        bytes[98] = 7; // Opponent penalties are not HULKs statistics.
        assert!(
            receiver
                .accept(parse(&bytes).unwrap().unwrap(), source(), now, wall_time())
                .is_none()
        );
        bytes[5] += 1;
        bytes[28] = 2;
        let changed = receiver
            .accept(parse(&bytes).unwrap().unwrap(), source(), now, wall_time())
            .unwrap();
        assert_eq!(changed.previous.unwrap().hulks_penalties, [0; 20]);
        assert_eq!(changed.snapshot.hulks_penalties[0], 2);
        assert!(initial.same_match_state(&changed.snapshot));
        assert!(!initial.same_game_state(&changed.snapshot));
        bytes[5] += 1;
        bytes[29] -= 1;
        bytes[30] += 1;
        assert!(
            receiver
                .accept(parse(&bytes).unwrap().unwrap(), source(), now, wall_time())
                .is_none()
        );
        assert_eq!(receiver.latest.unwrap().hulks_penalties[0], 2);
    }

    #[test]
    fn baseline_duplicates_and_clock_only_updates_are_quiet() {
        let mut receiver = receiver();
        let now = Instant::now();
        let initial = receiver
            .accept(snapshot(10), source(), now, wall_time())
            .unwrap();
        assert!(initial.previous.is_none());
        assert_eq!(initial.snapshot.hulks_score, 3);
        assert_eq!(initial.received_at, wall_time());
        assert!(
            receiver
                .accept(snapshot(10), source(), now, wall_time())
                .is_none()
        );
        let mut clock = snapshot(11);
        clock.seconds_remaining -= 1;
        clock.secondary_time -= 1;
        let later = now + Duration::from_secs(1);
        assert!(
            receiver
                .accept(clock.clone(), source(), later, wall_time())
                .is_none()
        );
        assert_eq!(receiver.latest, Some(clock));
        assert_eq!(receiver.received_at, Some(later));
        assert!(!receiver.is_stale_at(later));
    }

    #[test]
    fn sequence_wrap_reordering_and_reconnect() {
        let mut receiver = receiver();
        let now = Instant::now();
        receiver
            .accept(snapshot(255), source(), now, wall_time())
            .unwrap();
        assert!(
            receiver
                .accept(snapshot(0), source(), now, wall_time())
                .is_none()
        );
        assert_eq!(receiver.latest.as_ref().unwrap().packet_number, 0);
        for old in [255, 0, 128] {
            let mut data = snapshot(old);
            data.hulks_score = 8;
            assert!(receiver.accept(data, source(), now, wall_time()).is_none());
        }
        assert_eq!(receiver.latest.as_ref().unwrap().hulks_score, 3);
        let later = now + Duration::from_secs(6);
        assert!(receiver.is_stale_at(later));
        let resumed = receiver
            .accept(snapshot(0), source(), later, wall_time())
            .unwrap();
        assert!(resumed.previous.is_none());
    }

    #[test]
    fn sequence_gaps_rebaseline_unchanged_state_including_wrapping_gaps() {
        let now = Instant::now();
        for start in [0_u8, 255] {
            for delta in [2_u8, 127] {
                let mut receiver = receiver();
                receiver
                    .accept(snapshot(start), source(), now, wall_time())
                    .unwrap();
                let sequence = start.wrapping_add(delta);
                let update = receiver
                    .accept(snapshot(sequence), source(), now, wall_time())
                    .unwrap();
                assert!(update.previous.is_none());
                assert_eq!(update.snapshot.packet_number, sequence);
                assert!(
                    receiver
                        .accept(
                            snapshot(sequence.wrapping_add(1)),
                            source(),
                            now,
                            wall_time()
                        )
                        .is_none()
                );
                assert!(
                    receiver
                        .accept(snapshot(sequence), source(), now, wall_time())
                        .is_none()
                );
            }
        }
    }

    #[test]
    fn locks_source_and_opponent_even_after_rebaseline() {
        let mut receiver = receiver();
        let now = Instant::now();
        receiver
            .accept(snapshot(0), source(), now, wall_time())
            .unwrap();
        receiver.rebaseline();
        assert!(receiver.latest.is_none());
        assert!(receiver.is_stale());
        let other_source = "127.0.0.1:50001".parse().unwrap();
        assert!(
            receiver
                .accept(snapshot(1), other_source, now, wall_time())
                .is_none()
        );
        assert!(receiver.notice.as_ref().unwrap().contains("source"));
        let mut other_team = snapshot(1);
        other_team.opponent_number = 33;
        assert!(
            receiver
                .accept(other_team, source(), now, wall_time())
                .is_none()
        );
        assert!(receiver.notice.as_ref().unwrap().contains("opponent"));
        let update = receiver
            .accept(snapshot(0), source(), now, wall_time())
            .unwrap();
        assert!(update.previous.is_none());
        assert_eq!(receiver.source, Some(source()));
        assert!(receiver.notice.is_none());
    }

    #[test]
    fn feed_toggle_requires_a_source_and_preserves_game_pins() {
        let mut receiver = receiver();
        assert!(!receiver.true_data);
        assert_eq!(
            receiver.set_true_data(true).unwrap_err().kind(),
            io::ErrorKind::NotConnected
        );
        assert!(!receiver.true_data);
        let now = Instant::now();
        receiver
            .accept(snapshot(100), source(), now, wall_time())
            .unwrap();
        receiver.allow_opponent_change = true;
        receiver.ignored_opponent = Some(9);
        receiver.set_true_data(true).unwrap();
        assert!(receiver.true_data);
        assert!(receiver.latest.is_none());
        assert!(receiver.received_at.is_none());
        assert!(receiver.last_true_data_request.is_none());
        assert_eq!(receiver.source, Some(source()));
        assert_eq!(receiver.opponent_number, Some(5));
        assert!(receiver.allow_opponent_change);
        assert_eq!(receiver.ignored_opponent, Some(9));
        let true_source = "127.0.0.1:50001".parse().unwrap();
        let update = receiver
            .accept(snapshot(0), true_source, now, wall_time())
            .unwrap();
        assert!(update.previous.is_none());
        receiver.set_true_data(true).unwrap();
        assert_eq!(receiver.latest, Some(snapshot(0)));
        assert_eq!(receiver.true_source, Some(true_source));
        receiver.set_true_data(false).unwrap();
        assert!(receiver.latest.is_none());
        assert!(receiver.received_at.is_none());
        assert_eq!(receiver.source, Some(source()));
        assert!(receiver.allow_opponent_change);
        assert_eq!(receiver.ignored_opponent, Some(9));
        assert!(
            receiver
                .accept(snapshot(0), true_source, now, wall_time())
                .is_none()
        );
        assert!(
            receiver
                .accept(snapshot(0), source(), now, wall_time())
                .unwrap()
                .previous
                .is_none()
        );
    }

    #[test]
    fn true_feed_pins_a_separate_port_but_never_a_different_controller_ip() {
        let mut receiver = receiver();
        let now = Instant::now();
        receiver
            .accept(snapshot(50), source(), now, wall_time())
            .unwrap();
        receiver.set_true_data(true).unwrap();
        let another_ip = "127.0.0.2:50001".parse().unwrap();
        assert!(
            receiver
                .accept(snapshot(0), another_ip, now, wall_time())
                .is_none()
        );
        assert!(receiver.true_source.is_none());
        let true_source = "127.0.0.1:50001".parse().unwrap();
        let mut other_game = snapshot(0);
        other_game.opponent_number = 9;
        assert!(
            receiver
                .accept(other_game.clone(), true_source, now, wall_time())
                .is_none()
        );
        assert!(receiver.true_source.is_none());
        receiver.ignored_opponent = Some(5);
        assert!(
            receiver
                .accept(snapshot(0), true_source, now, wall_time())
                .is_none()
        );
        assert!(receiver.true_source.is_none());
        receiver.ignored_opponent = None;
        let update = receiver
            .accept(snapshot(255), true_source, now, wall_time())
            .unwrap();
        assert!(update.previous.is_none());
        assert!(
            receiver
                .accept(snapshot(0), source(), now, wall_time())
                .is_none()
        );
        receiver.rebaseline();
        assert!(
            receiver
                .accept(snapshot(0), source(), now, wall_time())
                .is_none()
        );
        assert!(
            receiver
                .accept(snapshot(255), true_source, now, wall_time())
                .unwrap()
                .previous
                .is_none()
        );
        assert!(
            receiver
                .accept(snapshot(0), true_source, now, wall_time())
                .is_none()
        );
        assert_eq!(receiver.latest.as_ref().unwrap().packet_number, 0);
        assert!(
            receiver
                .accept(snapshot(255), true_source, now, wall_time())
                .is_none()
        );
        receiver.allow_opponent_change = true;
        receiver.ignored_opponent = Some(9);
        assert!(
            receiver
                .accept(other_game.clone(), true_source, now, wall_time())
                .is_none()
        );
        receiver.ignored_opponent = None;
        assert!(
            receiver
                .accept(other_game, true_source, now, wall_time())
                .unwrap()
                .previous
                .is_none()
        );
        assert!(!receiver.allow_opponent_change);
        assert!(
            receiver
                .accept(snapshot(1), true_source, now, wall_time())
                .is_none()
        );
        assert_eq!(receiver.source, Some(source()));
        assert_eq!(receiver.opponent_number, Some(9));
    }

    #[test]
    fn true_requests_are_paced_only_while_waiting_or_silent_and_rebaseline_retries() {
        let mut receiver = receiver();
        let endpoint = request_endpoint(&mut receiver);
        let now = Instant::now();
        receiver
            .accept(snapshot(100), source(), now, wall_time())
            .unwrap();
        receiver.request_true_data(now).unwrap();
        assert_no_datagram(&endpoint);
        assert!(receiver.last_true_data_request.is_none());
        receiver.set_true_data(true).unwrap();
        assert_no_datagram(&endpoint);
        receiver.request_true_data(now).unwrap();
        assert_request(&endpoint, &receiver);
        receiver
            .request_true_data(now + Duration::from_millis(999))
            .unwrap();
        assert_no_datagram(&endpoint);
        receiver
            .request_true_data(now + TRUE_DATA_RETRY_AFTER)
            .unwrap();
        assert_request(&endpoint, &receiver);
        let true_source = "127.0.0.1:50001".parse().unwrap();
        for sequence in 0..8 {
            let received =
                now + Duration::from_secs(1) + Duration::from_millis(500 * u64::from(sequence));
            receiver.accept(snapshot(sequence), true_source, received, wall_time());
            receiver
                .request_true_data(received + Duration::from_millis(499))
                .unwrap();
            assert_no_datagram(&endpoint);
        }
        let retry = now + Duration::from_millis(5500);
        assert!(!receiver.is_stale_at(retry));
        receiver.request_true_data(retry).unwrap();
        assert_request(&endpoint, &receiver);
        let restarted_source = "127.0.0.1:50003".parse().unwrap();
        assert!(
            receiver
                .accept(snapshot(0), restarted_source, retry, wall_time())
                .unwrap()
                .previous
                .is_none()
        );
        let last_received = receiver.received_at;
        let latest = receiver.latest.clone();
        let stale = now + Duration::from_secs(11);
        assert!(receiver.is_stale_at(stale));
        receiver.request_true_data(stale).unwrap();
        assert_request(&endpoint, &receiver);
        assert_eq!(receiver.received_at, last_received);
        assert_eq!(receiver.latest, latest);
        assert!(receiver.true_source.is_none());
        assert!(receiver.is_stale_at(stale));
        let new_source = "127.0.0.1:50002".parse().unwrap();
        let mut resumed = snapshot(0);
        resumed.hulks_score = 8;
        resumed.seconds_remaining = 200;
        assert!(
            receiver
                .accept(resumed.clone(), new_source, stale, wall_time())
                .unwrap()
                .previous
                .is_none()
        );
        assert_eq!(receiver.latest, Some(resumed));
        assert!(!receiver.is_stale_at(stale));
        assert!(
            receiver
                .accept(snapshot(8), true_source, stale, wall_time())
                .is_none()
        );
        receiver.set_true_data(false).unwrap();
        receiver
            .request_true_data(stale + Duration::from_secs(20))
            .unwrap();
        assert_no_datagram(&endpoint);
        assert!(receiver.last_true_data_request.is_none());
    }

    #[test]
    fn retry_rejects_the_retired_port_even_before_the_new_publisher_arrives() {
        let mut receiver = receiver();
        receiver.enable_capture(50);
        let endpoint = request_endpoint(&mut receiver);
        let old_sender = UdpSocket::bind("127.0.0.1:0").unwrap();
        let new_sender = UdpSocket::bind("127.0.0.1:0").unwrap();
        let destination = receiver.socket.local_addr().unwrap();
        receiver
            .accept(snapshot(100), source(), Instant::now(), wall_time())
            .unwrap();
        receiver.set_true_data(true).unwrap();
        old_sender.send_to(&true_packet(100), destination).unwrap();
        assert_eq!(poll_when_ready(&mut receiver).len(), 1);
        let latest = receiver.latest.clone();
        let received = receiver.received_at;
        let retry = received.unwrap() + TRUE_DATA_RETRY_AFTER;
        receiver.request_true_data(retry).unwrap();
        assert_request(&endpoint, &receiver);

        let mut late = true_packet(101);
        late[10] = 4;
        late[22] = 9;
        old_sender.send_to(&late, destination).unwrap();
        assert!(poll_when_ready(&mut receiver).is_empty());
        assert_eq!(receiver.latest, latest);
        assert_eq!(receiver.received_at, received);
        assert!(receiver.true_source.is_none());

        receiver
            .request_true_data(retry + TRUE_DATA_RETRY_AFTER)
            .unwrap();
        assert_request(&endpoint, &receiver);
        // A reused port is indistinguishable from a delayed old sequence zero.
        old_sender.send_to(&true_packet(0), destination).unwrap();
        assert!(poll_when_ready(&mut receiver).is_empty());
        assert!(receiver.true_source.is_none());

        let mut current = true_packet(0);
        current[22] = 4;
        new_sender.send_to(&current, destination).unwrap();
        let updates = poll_when_ready(&mut receiver);
        assert_eq!(updates.len(), 1);
        assert!(updates[0].previous.is_none());
        assert_eq!(updates[0].snapshot.hulks_score, 4);
        assert_eq!(receiver.true_source, Some(new_sender.local_addr().unwrap()));
        assert_eq!(updates[0].capture_id, Some(53));
        assert_eq!(receiver.captures[3].packet, current);
        assert_eq!(
            receiver.captures[3].source,
            new_sender.local_addr().unwrap()
        );
        old_sender.send_to(&late, destination).unwrap();
        assert!(poll_when_ready(&mut receiver).is_empty());
        assert_eq!(receiver.latest.as_ref().unwrap().hulks_score, 4);
        assert_eq!(receiver.captures.len(), 5);
        for (index, capture) in receiver.captures.iter().enumerate() {
            assert_eq!(capture.id, 50 + index as u64);
            assert!(capture.true_data);
            assert_eq!(capture.accepted, index == 0 || index == 3);
            assert_eq!(capture.baseline, capture.accepted);
        }
    }

    #[test]
    fn log_rebaseline_preserves_true_transport_silence_for_retry_pacing() {
        let mut receiver = receiver();
        let endpoint = request_endpoint(&mut receiver);
        let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
        let true_source = sender.local_addr().unwrap();
        let now = Instant::now();
        receiver
            .accept(snapshot(100), source(), now, wall_time())
            .unwrap();
        receiver.set_true_data(true).unwrap();
        receiver.request_true_data(now).unwrap();
        assert_request(&endpoint, &receiver);
        let received = now + Duration::from_secs(2);
        receiver
            .accept(snapshot(0), true_source, received, wall_time())
            .unwrap();
        receiver.rebaseline();
        assert!(receiver.latest.is_none());
        assert!(receiver.received_at.is_none());
        receiver
            .request_true_data(received + Duration::from_millis(999))
            .unwrap();
        assert_no_datagram(&endpoint);
        let resumed = received + TRUE_DATA_RETRY_AFTER;
        assert!(
            receiver
                .accept(snapshot(1), true_source, resumed, wall_time())
                .unwrap()
                .previous
                .is_none()
        );
        receiver.rebaseline();
        receiver
            .request_true_data(resumed + Duration::from_millis(999))
            .unwrap();
        assert_no_datagram(&endpoint);
        receiver
            .request_true_data(resumed + TRUE_DATA_RETRY_AFTER)
            .unwrap();
        assert_request(&endpoint, &receiver);
    }

    #[test]
    fn request_failure_is_visible_paced_and_retried_without_disabling_true_mode() {
        let mut receiver = receiver();
        receiver
            .accept(snapshot(0), source(), Instant::now(), wall_time())
            .unwrap();
        receiver.set_true_data(true).unwrap();
        assert!(receiver.poll().unwrap().is_empty());
        assert!(
            receiver
                .notice
                .as_ref()
                .unwrap()
                .contains("true-data request failed")
        );
        assert!(receiver.true_data);
        assert!(receiver.latest.is_none());
        let first_attempt = receiver.last_true_data_request.unwrap();
        receiver
            .request_true_data(first_attempt + Duration::from_millis(999))
            .unwrap();
        assert_eq!(receiver.last_true_data_request, Some(first_attempt));
        assert!(
            receiver
                .request_true_data(first_attempt + Duration::from_secs(1))
                .is_err()
        );
        let endpoint = request_endpoint(&mut receiver);
        let recovered = first_attempt + Duration::from_secs(2);
        receiver.request_true_data(recovered).unwrap();
        assert_request(&endpoint, &receiver);
        receiver
            .accept(snapshot(0), source(), recovered, wall_time())
            .unwrap();
        assert!(receiver.notice.is_none());
        assert!(receiver.true_data);
    }

    #[test]
    fn permitted_opponent_change_rebaselines_once_without_unlocking_source() {
        let mut receiver = receiver();
        let now = Instant::now();
        receiver
            .accept(snapshot(100), source(), now, wall_time())
            .unwrap();
        receiver.allow_opponent_change = true;
        let mut official = snapshot(0);
        official.opponent_number = 9;
        official.hulks_score = 0;
        let other_source = "127.0.0.1:50001".parse().unwrap();
        assert!(
            receiver
                .accept(official.clone(), other_source, now, wall_time())
                .is_none()
        );
        assert!(receiver.allow_opponent_change);
        let baseline = receiver
            .accept(official.clone(), source(), now, wall_time())
            .unwrap();
        assert!(baseline.previous.is_none());
        assert_eq!(receiver.source, Some(source()));
        assert!(!receiver.allow_opponent_change);
        assert!(
            receiver
                .accept(snapshot(101), source(), now, wall_time())
                .is_none()
        );
        assert!(
            receiver
                .accept(official.clone(), source(), now, wall_time())
                .is_none()
        );
        official.packet_number = 255;
        official.hulks_score = 1;
        assert!(
            receiver
                .accept(official.clone(), source(), now, wall_time())
                .is_none()
        );
        official.packet_number = 1;
        assert!(
            receiver
                .accept(official.clone(), source(), now, wall_time())
                .unwrap()
                .previous
                .is_some()
        );
        let later = now + Duration::from_secs(6);
        assert!(
            receiver
                .accept(official, source(), later, wall_time())
                .unwrap()
                .previous
                .is_none()
        );
        assert!(
            receiver
                .accept(snapshot(102), source(), later, wall_time())
                .is_none()
        );
    }

    #[test]
    fn ignored_test_opponent_cannot_pin_a_reopened_receiver() {
        let mut receiver = receiver();
        receiver.ignored_opponent = Some(5);
        let now = Instant::now();
        assert!(
            receiver
                .accept(snapshot(0), source(), now, wall_time())
                .is_none()
        );
        assert!(receiver.source.is_none());
        assert!(receiver.latest.is_none());
        let mut official = snapshot(0);
        official.opponent_number = 9;
        assert!(
            receiver
                .accept(official, source(), now, wall_time())
                .unwrap()
                .previous
                .is_none()
        );
        assert_eq!(receiver.opponent_number, Some(9));
    }

    #[test]
    fn reopened_transition_pins_first_official_packet_against_late_test_packets() {
        let mut receiver = receiver();
        receiver.allow_opponent_change = true;
        let now = Instant::now();
        let mut official = snapshot(0);
        official.opponent_number = 9;
        assert!(
            receiver
                .accept(official.clone(), source(), now, wall_time())
                .unwrap()
                .previous
                .is_none()
        );
        assert!(!receiver.allow_opponent_change);
        assert!(
            receiver
                .accept(snapshot(100), source(), now, wall_time())
                .is_none()
        );
        assert_eq!(receiver.latest, Some(official));
    }

    #[test]
    fn queued_official_transition_cannot_repin_before_the_action_menu_closes() {
        for true_data in [false, true] {
            use crate::{
                app::{App, Mode},
                model::{EventKind, MatchType, Metadata, Session},
            };
            let mut app = App::new(None).unwrap();
            let mut session = Session::new_match(
                Metadata {
                    event: "test".into(),
                    home: "HULKs".into(),
                    away: "HTWK".into(),
                    date: wall_time().date_naive(),
                    label: "game".into(),
                    robots: std::array::from_fn(|_| String::new()),
                },
                MatchType::Combined,
            );
            session.golden_goal_opponent = Some(5);
            session.record(EventKind::GameEnd, wall_time());
            app.session = Some(session);
            app.mode = Mode::Actions {
                selected: 0,
                timestamp: wall_time(),
                message_budget: None,
                gc_seconds_remaining: None,
            };
            let mut receiver = receiver();
            let now = Instant::now();
            receiver
                .accept(snapshot(100), source(), now, wall_time())
                .unwrap();
            if true_data {
                receiver.set_true_data(true).unwrap();
                receiver
                    .accept(snapshot(100), source(), now, wall_time())
                    .unwrap();
            }
            app.controller = Some(receiver);
            app.poll_controller();
            let mut official = snapshot(0);
            official.opponent_number = 9;
            official.state = 1;
            official.hulks_score = 0;
            official.opponent_score = 0;
            let update = app
                .controller
                .as_mut()
                .unwrap()
                .accept(official.clone(), source(), now, wall_time())
                .unwrap();
            app.pending_controller.push(update);
            app.poll_controller();
            assert_eq!(app.pending_controller.len(), 1);
            let receiver = app.controller.as_mut().unwrap();
            assert!(!receiver.allow_opponent_change);
            let mut unrelated = official.clone();
            unrelated.opponent_number = 10;
            assert!(
                receiver
                    .accept(unrelated, source(), now, wall_time())
                    .is_none()
            );
            assert!(
                receiver
                    .accept(snapshot(101), source(), now, wall_time())
                    .is_none()
            );
            assert_eq!(receiver.latest, Some(official));
            app.mode = Mode::Idle;
            app.poll_controller();
            assert_eq!(
                app.session.as_ref().unwrap().current_game(),
                MatchType::Official
            );
            assert_eq!(app.session.as_ref().unwrap().score(), (0, 0));
        }
    }

    #[test]
    fn allowing_an_opponent_change_does_not_disable_same_game_sequence_checks() {
        let mut receiver = receiver();
        let now = Instant::now();
        receiver
            .accept(snapshot(100), source(), now, wall_time())
            .unwrap();
        receiver.allow_opponent_change = true;
        let mut restart = snapshot(0);
        restart.hulks_score = 0;
        assert!(
            receiver
                .accept(restart, source(), now, wall_time())
                .is_none()
        );
        assert_eq!(receiver.latest.as_ref().unwrap().hulks_score, 3);
        assert!(receiver.allow_opponent_change);
    }

    #[test]
    fn budget_only_changes_update_latest_without_generating_events() {
        let mut receiver = receiver();
        let now = Instant::now();
        assert!(
            receiver
                .accept(snapshot(0), source(), now, wall_time())
                .is_some()
        );
        let mut bytes = packet(1);
        bytes[26..28].copy_from_slice(&60_000_u16.to_le_bytes());
        let changed = parse(&bytes).unwrap().unwrap();
        assert_eq!(changed.hulks_message_budget, 60_000);
        assert!(
            receiver
                .accept(changed, source(), now + Duration::from_secs(1), wall_time())
                .is_none()
        );
        assert_eq!(
            receiver.latest.as_ref().unwrap().hulks_message_budget,
            60_000
        );
    }

    #[test]
    fn score_decreases_and_each_semantic_change_emit_diffs() {
        let now = Instant::now();
        for field in 0..9 {
            let mut receiver = receiver();
            receiver
                .accept(snapshot(0), source(), now, wall_time())
                .unwrap();
            let mut changed = snapshot(1);
            match field {
                0 => changed.hulks_score -= 1,
                1 => changed.opponent_score += 1,
                2 => changed.game_phase = 0,
                3 => changed.state = 4,
                4 => changed.set_play = 0,
                5 => changed.first_half = false,
                6 => changed.stopped = false,
                7 => changed.kicking_team = 5,
                8 => changed.kicking_team = 255,
                _ => unreachable!(),
            }
            let update = receiver
                .accept(changed.clone(), source(), now, wall_time())
                .unwrap();
            assert_eq!(update.previous, Some(snapshot(0)));
            assert_eq!(update.snapshot, changed);
        }
    }

    fn poll_when_ready(receiver: &mut Receiver) -> Vec<Update> {
        receiver.socket.set_nonblocking(false).unwrap();
        receiver
            .socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        receiver.socket.peek_from(&mut [0; 2048]).unwrap();
        receiver.socket.set_nonblocking(true).unwrap();
        receiver.poll().unwrap()
    }

    #[test]
    fn capture_is_opt_in_and_ids_survive_draining() {
        let mut receiver = receiver();
        let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
        let destination = receiver.socket.local_addr().unwrap();
        sender.send_to(&packet(0), destination).unwrap();
        let updates = poll_when_ready(&mut receiver);
        assert_eq!(updates[0].capture_id, None);
        assert!(receiver.captures.is_empty());
        assert_eq!(receiver.next_capture_id(), 0);

        receiver.enable_capture(40);
        sender.send_to(&packet(1), destination).unwrap();
        assert!(poll_when_ready(&mut receiver).is_empty());
        let captures = std::mem::take(&mut receiver.captures);
        assert_eq!(captures[0].id, 40);
        assert!(captures[0].accepted);
        assert!(!captures[0].baseline);
        assert_eq!(receiver.next_capture_id(), 41);

        sender.send_to(b"not GameController", destination).unwrap();
        assert!(poll_when_ready(&mut receiver).is_empty());
        assert!(receiver.captures.is_empty());
        assert_eq!(receiver.next_capture_id(), 41);
        sender.send_to(&packet(3), destination).unwrap();
        let updates = poll_when_ready(&mut receiver);
        assert_eq!(updates[0].capture_id, Some(41));
        assert!(receiver.captures[0].accepted);
        assert!(receiver.captures[0].baseline);
        assert_eq!(receiver.next_capture_id(), 42);

        let mut changed = snapshot(4);
        changed.hulks_score += 1;
        let update = receiver
            .accept(
                changed,
                sender.local_addr().unwrap(),
                Instant::now(),
                wall_time(),
            )
            .unwrap();
        assert_eq!(update.capture_id, None);
        assert_eq!(receiver.next_capture_id(), 42);
    }

    #[test]
    fn capture_keeps_both_feeds_and_distinguishes_acceptance_from_updates() {
        for true_data in [false, true] {
            let mut receiver = receiver();
            receiver.enable_capture(10);
            let endpoint = request_endpoint(&mut receiver);
            let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
            let other_port = UdpSocket::bind("127.0.0.1:0").unwrap();
            let destination = receiver.socket.local_addr().unwrap();
            if true_data {
                receiver.source = Some(sender.local_addr().unwrap());
                receiver.set_true_data(true).unwrap();
            }
            let mut selected = if true_data { true_packet(0) } else { packet(0) };
            let before = Local::now().fixed_offset();
            sender.send_to(&selected, destination).unwrap();
            let updates = poll_when_ready(&mut receiver);
            let after = Local::now().fixed_offset();
            assert_eq!(updates.len(), 1);
            let capture = &receiver.captures[0];
            assert_eq!(capture.id, 10);
            assert_eq!(updates[0].capture_id, Some(capture.id));
            assert_eq!(capture.received_at, updates[0].received_at);
            assert!((before..=after).contains(&capture.received_at));
            assert_eq!(capture.source, sender.local_addr().unwrap());
            assert_eq!(capture.packet, selected);
            assert_eq!(capture.true_data, true_data);
            assert!(capture.accepted);
            assert!(capture.baseline);
            assert!(capture.game.is_none());
            assert!(!capture.active);
            assert_eq!(
                decode_capture(&capture.packet, true_data).unwrap(),
                Some(snapshot(0))
            );

            let ignored = if true_data { packet(0) } else { true_packet(0) };
            other_port.send_to(&ignored, destination).unwrap();
            assert!(poll_when_ready(&mut receiver).is_empty());
            let capture = &receiver.captures[1];
            assert_eq!(capture.packet, ignored);
            assert_eq!(capture.source, other_port.local_addr().unwrap());
            assert_eq!(capture.true_data, true_data);
            assert!(!capture.accepted);
            assert!(!capture.baseline);
            assert_eq!(decode_capture(&capture.packet, true_data).unwrap(), None);

            selected[5] = 1;
            selected[14] -= 1;
            sender.send_to(&selected, destination).unwrap();
            assert!(poll_when_ready(&mut receiver).is_empty());
            assert!(receiver.captures[2].accepted);
            assert!(!receiver.captures[2].baseline);
            assert_eq!(receiver.latest.as_ref().unwrap().seconds_remaining, -314);

            selected[5] = 2;
            selected[22] += 1;
            sender.send_to(&selected, destination).unwrap();
            let updates = poll_when_ready(&mut receiver);
            assert_eq!(updates[0].capture_id, Some(13));
            assert!(updates[0].previous.is_some());
            assert!(receiver.captures[3].accepted);
            assert!(!receiver.captures[3].baseline);

            for sequence in [2, 1] {
                selected[5] = sequence;
                sender.send_to(&selected, destination).unwrap();
                assert!(poll_when_ready(&mut receiver).is_empty());
                assert!(!receiver.captures.last().unwrap().accepted);
            }
            selected[5] = 3;
            selected[88] = 14;
            sender.send_to(&selected, destination).unwrap();
            assert!(poll_when_ready(&mut receiver).is_empty());
            assert!(!receiver.captures.last().unwrap().accepted);
            selected[88] = 5;
            other_port.send_to(&selected, destination).unwrap();
            assert!(poll_when_ready(&mut receiver).is_empty());
            assert!(!receiver.captures.last().unwrap().accepted);
            assert_eq!(
                receiver.captures.last().unwrap().source,
                other_port.local_addr().unwrap()
            );
            assert_eq!(
                receiver
                    .captures
                    .iter()
                    .map(|capture| capture.id)
                    .collect::<Vec<_>>(),
                (10..18).collect::<Vec<_>>()
            );
            assert_eq!(receiver.next_capture_id(), 18);
            assert_no_datagram(&endpoint);
        }
    }

    #[test]
    fn capture_keeps_malformed_payloads_and_marks_the_next_baseline() {
        for true_data in [false, true] {
            let mut receiver = receiver();
            receiver.enable_capture(0);
            let _endpoint = request_endpoint(&mut receiver);
            let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
            let destination = receiver.socket.local_addr().unwrap();
            if true_data {
                receiver.source = Some(sender.local_addr().unwrap());
                receiver.set_true_data(true).unwrap();
            }
            let mut selected = if true_data { true_packet(0) } else { packet(0) };
            sender.send_to(&selected, destination).unwrap();
            assert_eq!(poll_when_ready(&mut receiver).len(), 1);
            let mut old_version = selected.to_vec();
            old_version[4] = 19;
            let mut bad_state = selected.to_vec();
            bad_state[10] = 255;
            let mut overlong = selected.to_vec();
            overlong.extend((0..6000).map(|index| (index % 251) as u8));
            let mut ignored = overlong.clone();
            ignored[..4].copy_from_slice(if true_data { b"RGme" } else { b"RGTD" });
            sender.send_to(&ignored, destination).unwrap();
            assert!(poll_when_ready(&mut receiver).is_empty());
            assert_eq!(receiver.captures[1].packet, ignored);
            assert!(!receiver.captures[1].accepted);
            assert!(receiver.latest.is_some());
            for invalid in [selected[..4].to_vec(), old_version, bad_state, overlong] {
                sender.send_to(&invalid, destination).unwrap();
                assert!(poll_when_ready(&mut receiver).is_empty());
                let capture = receiver.captures.last().unwrap();
                assert_eq!(capture.packet, invalid);
                assert_eq!(capture.true_data, true_data);
                assert!(!capture.accepted);
                assert!(!capture.baseline);
                assert!(decode_capture(&capture.packet, true_data).is_err());
                assert!(receiver.latest.is_none());

                selected[5] += 1;
                sender.send_to(&selected, destination).unwrap();
                let updates = poll_when_ready(&mut receiver);
                assert!(updates[0].previous.is_none());
                let capture = receiver.captures.last().unwrap();
                assert_eq!(updates[0].capture_id, Some(capture.id));
                assert!(capture.accepted);
                assert!(capture.baseline);
            }
            assert_eq!(receiver.captures.len(), 10);
            assert_eq!(receiver.next_capture_id(), 10);
        }
    }

    #[test]
    fn capture_ignores_foreign_ips_and_unrelated_pre_discovery_packets() {
        let mut receiver = receiver();
        receiver.enable_capture(8);
        let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
        let destination = receiver.socket.local_addr().unwrap();
        let mut unrelated = packet(0);
        unrelated[18] = 33;
        for bytes in [&unrelated[..], &true_packet(0), b"RGme"] {
            sender.send_to(bytes, destination).unwrap();
            assert!(poll_when_ready(&mut receiver).is_empty());
        }
        assert!(receiver.source.is_none());
        assert!(receiver.captures.is_empty());
        assert_eq!(receiver.next_capture_id(), 8);

        // Simulate a foreign IP without requiring a non-loopback test destination.
        receiver.source = Some("192.0.2.1:50000".parse().unwrap());
        for bytes in [&packet(0)[..], &true_packet(0), b"RGme"] {
            sender.send_to(bytes, destination).unwrap();
            assert!(poll_when_ready(&mut receiver).is_empty());
        }
        assert!(receiver.captures.is_empty());
        assert_eq!(receiver.next_capture_id(), 8);

        receiver.source = None;
        sender.send_to(&packet(0), destination).unwrap();
        assert_eq!(poll_when_ready(&mut receiver)[0].capture_id, Some(8));
        sender.send_to(&unrelated, destination).unwrap();
        assert!(poll_when_ready(&mut receiver).is_empty());
        assert_eq!(receiver.captures[1].id, 9);
        assert_eq!(receiver.captures[1].packet, unrelated);
        assert!(!receiver.captures[1].accepted);
        assert!(!receiver.captures[1].baseline);
    }

    #[test]
    fn loopback_receives_passively_and_unrelated_teams_cannot_pin_source() {
        let mut receiver = receiver();
        let destination = receiver.socket.local_addr().unwrap();
        let unrelated = UdpSocket::bind("127.0.0.1:0").unwrap();
        let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
        let mut other_game = packet(0);
        other_game[18] = 33;
        unrelated.send_to(&other_game, destination).unwrap();
        assert!(poll_when_ready(&mut receiver).is_empty());
        assert!(receiver.source.is_none());
        sender.send_to(&packet(0), destination).unwrap();
        assert_eq!(poll_when_ready(&mut receiver).len(), 1);
        let mut clock_only = packet(1);
        clock_only[14] -= 1;
        clock_only[26] = 99;
        clock_only[29] = 4; // Penalty countdown, not its reason code.
        sender.send_to(&clock_only, destination).unwrap();
        assert!(poll_when_ready(&mut receiver).is_empty());
        assert_eq!(receiver.source, Some(sender.local_addr().unwrap()));
        assert_eq!(receiver.latest.as_ref().unwrap().packet_number, 1);
        assert_eq!(receiver.latest.as_ref().unwrap().seconds_remaining, -314);
        let mut penalty = clock_only;
        penalty[5] = 2;
        penalty[28] = 4;
        sender.send_to(&penalty, destination).unwrap();
        let updates = poll_when_ready(&mut receiver);
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].snapshot.hulks_penalties[0], 4);
        assert_eq!(updates[0].previous.as_ref().unwrap().hulks_penalties[0], 0);
        assert!(receiver.poll().unwrap().is_empty());
        sender.set_nonblocking(true).unwrap();
        assert_eq!(
            sender.recv_from(&mut [0; 8]).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
    }

    #[test]
    fn wire_coverage_loss_syncs_scores_and_marks_statistics_partial() {
        use crate::{
            app::App,
            model::{EventKind, MatchType, Metadata, Session},
        };
        for true_data in [false, true] {
            for malformed in [false, true] {
                for score in [0, 1] {
                    let mut receiver = receiver();
                    let endpoint = request_endpoint(&mut receiver);
                    let destination = receiver.socket.local_addr().unwrap();
                    let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
                    let foreign = UdpSocket::bind("127.0.0.1:0").unwrap();
                    if true_data {
                        receiver.source = Some(foreign.local_addr().unwrap());
                        receiver.set_true_data(true).unwrap();
                    }
                    let mut app = App::new(None).unwrap();
                    app.session = Some(Session::new_match(
                        Metadata {
                            event: "test".into(),
                            home: "HULKs".into(),
                            away: "HTWK".into(),
                            date: wall_time().date_naive(),
                            label: "game".into(),
                            robots: std::array::from_fn(|_| String::new()),
                        },
                        MatchType::Official,
                    ));
                    let mut bytes = if true_data { true_packet(0) } else { packet(0) };
                    bytes[9] = 0;
                    bytes[10] = 0;
                    bytes[11] = 0;
                    bytes[22] = 0;
                    bytes[92] = 0;
                    sender.send_to(&bytes, destination).unwrap();
                    app.pending_controller = poll_when_ready(&mut receiver);
                    app.flush_controller();
                    bytes[5] = 1;
                    bytes[10] = 3;
                    sender.send_to(&bytes, destination).unwrap();
                    app.pending_controller = poll_when_ready(&mut receiver);
                    app.flush_controller();

                    if malformed {
                        let latest = receiver.latest.clone();
                        let receipt = receiver.received_at;
                        let true_receipt = receiver.last_true_receipt;
                        let source = receiver.source;
                        let true_source = receiver.true_source;
                        let opponent = receiver.opponent_number;
                        let mut invalid = bytes;
                        invalid[9] = 255;
                        // A foreign endpoint (even the true feed's broadcast source) cannot break coverage.
                        foreign.send_to(&invalid, destination).unwrap();
                        assert!(poll_when_ready(&mut receiver).is_empty());
                        assert_eq!(receiver.latest, latest);
                        invalid[..4].copy_from_slice(if true_data { b"RGme" } else { b"RGTD" });
                        sender.send_to(&invalid, destination).unwrap();
                        assert!(poll_when_ready(&mut receiver).is_empty());
                        assert_eq!(receiver.latest, latest);
                        assert_eq!(receiver.received_at, receipt);
                        invalid[..4].copy_from_slice(if true_data { b"RGTD" } else { b"RGme" });
                        sender.send_to(&invalid, destination).unwrap();
                        assert!(poll_when_ready(&mut receiver).is_empty());
                        assert!(receiver.notice.as_ref().unwrap().contains("enum"));
                        assert!(receiver.latest.is_none());
                        assert!(receiver.received_at.is_none());
                        assert_eq!(receiver.last_true_receipt, true_receipt);
                        assert_eq!(receiver.source, source);
                        assert_eq!(receiver.true_source, true_source);
                        assert_eq!(receiver.opponent_number, opponent);
                    }
                    // After malformed data use delta 1, so this cannot pass via gap detection alone.
                    bytes[5] += if malformed { 1 } else { 2 };
                    bytes[22] = score;
                    if score != 0 {
                        bytes[28] = 5;
                        bytes[11] = 5;
                        bytes[13] = bytes[88];
                    }
                    sender.send_to(&bytes, destination).unwrap();
                    let updates = poll_when_ready(&mut receiver);
                    assert_eq!(updates.len(), 1);
                    assert!(updates[0].previous.is_none());
                    app.pending_controller = updates;
                    app.flush_controller();
                    let session = app.session.as_ref().unwrap();
                    let entry = session.entries.last().unwrap();
                    assert_eq!(entry.kind, EventKind::Custom);
                    assert_eq!(
                        entry.score_delta,
                        (score != 0).then_some([i32::from(score), 0])
                    );
                    let statistics = entry.statistics.as_ref().unwrap();
                    assert!(!statistics.baseline.as_ref().unwrap().pre_game);
                    assert!(statistics.penalties.is_empty());
                    assert!(!statistics.opponent_restart);
                    assert_eq!(session.score(), (usize::from(score), 0));
                    bytes[5] += 1;
                    bytes[10] = 4;
                    bytes[12] = 0;
                    sender.send_to(&bytes, destination).unwrap();
                    app.pending_controller = poll_when_ready(&mut receiver);
                    app.flush_controller();
                    let markdown = app.session.as_ref().unwrap().markdown();
                    assert!(markdown.contains("partial GC coverage"));
                    assert!(markdown.contains("Goal share: unavailable"));
                    assert!(
                        markdown.contains("Penalties observed (total/incapable): none observed")
                    );
                    assert!(markdown.contains("0 observed opponent goal-kicks/kick-ins"));
                    assert_no_datagram(&endpoint);
                }
            }
        }
    }

    #[test]
    fn discovery_automatically_requests_true_data_without_importing_delayed_state() {
        let mut receiver = receiver();
        receiver.enable_capture(70);
        receiver.auto_true_data = true;
        let endpoint = request_endpoint(&mut receiver);
        let destination = receiver.socket.local_addr().unwrap();
        let broadcaster = UdpSocket::bind("127.0.0.1:0").unwrap();
        let true_sender = UdpSocket::bind("127.0.0.1:0").unwrap();
        assert!(receiver.poll().unwrap().is_empty());
        assert_no_datagram(&endpoint);
        let mut unrelated = packet(0);
        unrelated[18] = 33;
        broadcaster.send_to(&unrelated, destination).unwrap();
        assert!(poll_when_ready(&mut receiver).is_empty());
        assert_no_datagram(&endpoint);
        assert!(receiver.source.is_none());
        assert!(receiver.captures.is_empty());

        let mut delayed = packet(100);
        delayed[10] = 4; // A delayed Finished snapshot must not end the current log.
        delayed[22] = 9;
        broadcaster.send_to(&delayed, destination).unwrap();
        assert!(poll_when_ready(&mut receiver).is_empty());
        assert!(receiver.true_data);
        assert!(!receiver.auto_true_data);
        assert!(receiver.latest.is_none());
        assert!(receiver.received_at.is_none());
        assert_eq!(receiver.source, Some(broadcaster.local_addr().unwrap()));
        assert_request(&endpoint, &receiver);
        assert_eq!(receiver.captures.len(), 1);
        let capture = &receiver.captures[0];
        assert_eq!(capture.id, 70);
        assert_eq!(capture.packet, delayed);
        assert!(!capture.true_data);
        assert!(!capture.accepted);
        assert!(!capture.baseline);

        let mut current = true_packet(0);
        current[88] = 14; // True data may already describe the next opponent.
        true_sender.send_to(&current, destination).unwrap();
        let updates = poll_when_ready(&mut receiver);
        assert_eq!(updates.len(), 1);
        assert!(updates[0].previous.is_none());
        assert_eq!(updates[0].snapshot.opponent_number, 14);
        assert_eq!(updates[0].capture_id, Some(71));
        assert_eq!(receiver.captures[1].packet, current);
        assert!(receiver.captures[1].true_data);
        assert!(receiver.captures[1].accepted);
        assert!(receiver.captures[1].baseline);
        assert_eq!(updates[0].snapshot.state, 3);
        assert_eq!(updates[0].snapshot.hulks_score, 3);
        assert_no_datagram(&endpoint);

        receiver.set_true_data(false).unwrap();
        delayed[88] = 14;
        broadcaster.send_to(&delayed, destination).unwrap();
        assert_eq!(poll_when_ready(&mut receiver).len(), 1);
        receiver.rebaseline();
        delayed[5] = 101;
        broadcaster.send_to(&delayed, destination).unwrap();
        assert_eq!(poll_when_ready(&mut receiver).len(), 1);
        assert!(!receiver.true_data);
        assert_no_datagram(&endpoint);
    }

    #[test]
    fn automatic_true_data_waits_without_silent_broadcast_fallback() {
        let mut receiver = receiver();
        receiver.auto_true_data = true;
        let endpoint = request_endpoint(&mut receiver);
        let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
        let destination = receiver.socket.local_addr().unwrap();
        sender.send_to(&packet(0), destination).unwrap();
        assert!(poll_when_ready(&mut receiver).is_empty());
        assert_request(&endpoint, &receiver);
        sender.send_to(&packet(1), destination).unwrap();
        assert!(poll_when_ready(&mut receiver).is_empty());
        assert!(receiver.true_data);
        assert!(receiver.latest.is_none());
        assert_no_datagram(&endpoint);
    }

    #[test]
    fn loopback_feed_switches_never_mix_delayed_and_true_snapshots() {
        let mut receiver = receiver();
        let endpoint = request_endpoint(&mut receiver);
        let destination = receiver.socket.local_addr().unwrap();
        let broadcast_sender = UdpSocket::bind("127.0.0.1:0").unwrap();
        let true_sender = UdpSocket::bind("127.0.0.1:0").unwrap();
        true_sender.send_to(&true_packet(0), destination).unwrap();
        assert!(poll_when_ready(&mut receiver).is_empty());
        assert!(receiver.source.is_none());
        broadcast_sender.send_to(&packet(100), destination).unwrap();
        assert!(poll_when_ready(&mut receiver)[0].previous.is_none());
        assert_no_datagram(&endpoint);
        receiver.set_true_data(true).unwrap();
        assert_no_datagram(&endpoint);
        let mut delayed = packet(101);
        delayed[22] = 7;
        delayed[14..16].copy_from_slice(&400_i16.to_le_bytes());
        broadcast_sender.send_to(&delayed, destination).unwrap();
        assert!(poll_when_ready(&mut receiver).is_empty());
        assert_request(&endpoint, &receiver);
        assert!(receiver.latest.is_none());
        assert!(receiver.received_at.is_none());
        let mut current = true_packet(0);
        current[22] = 4;
        current[14..16].copy_from_slice(&200_i16.to_le_bytes());
        true_sender.send_to(&current, destination).unwrap();
        let updates = poll_when_ready(&mut receiver);
        assert_eq!(updates.len(), 1);
        assert!(updates[0].previous.is_none());
        assert_eq!(updates[0].snapshot.hulks_score, 4);
        assert_eq!(updates[0].snapshot.seconds_remaining, 200);
        let true_received = receiver.received_at;
        delayed[5] = 1;
        broadcast_sender.send_to(&delayed, destination).unwrap();
        assert!(poll_when_ready(&mut receiver).is_empty());
        assert_eq!(receiver.received_at, true_received);
        current[5] = 1;
        current[22] = 5;
        true_sender.send_to(&current, destination).unwrap();
        let updates = poll_when_ready(&mut receiver);
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].previous.as_ref().unwrap().hulks_score, 4);
        assert_eq!(updates[0].snapshot.hulks_score, 5);
        assert_eq!(
            receiver.source,
            Some(broadcast_sender.local_addr().unwrap())
        );
        receiver.set_true_data(false).unwrap();
        current[5] = 2;
        true_sender.send_to(&current, destination).unwrap();
        assert!(poll_when_ready(&mut receiver).is_empty());
        assert!(receiver.latest.is_none());
        assert!(receiver.received_at.is_none());
        assert_no_datagram(&endpoint);
        broadcast_sender.send_to(&delayed, destination).unwrap();
        let updates = poll_when_ready(&mut receiver);
        assert_eq!(updates.len(), 1);
        assert!(updates[0].previous.is_none());
        assert_eq!(updates[0].snapshot.packet_number, 1);
        assert_eq!(updates[0].snapshot.hulks_score, 7);
        assert_eq!(updates[0].snapshot.seconds_remaining, 400);
        assert_no_datagram(&endpoint);
    }

    fn capture_app(
        match_type: crate::model::MatchType,
    ) -> (tempfile::TempDir, crate::app::App, UdpSocket) {
        use crate::{
            app::{App, Mode},
            model::{Metadata, Session},
            storage::Store,
        };
        let directory = tempfile::tempdir().unwrap();
        let mut session = Session::new_match(
            Metadata {
                event: "capture integration".into(),
                home: "HULKs".into(),
                away: "B-Human".into(),
                date: wall_time().date_naive(),
                label: "game".into(),
                robots: std::array::from_fn(|_| String::new()),
            },
            match_type,
        );
        let store = Store::create(directory.path(), &session).unwrap();
        let capture = store.prepare_capture(&mut session).unwrap();
        let mut receiver = receiver();
        receiver.enable_capture(capture.next_id());
        let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
        let mut app = App::new(None).unwrap();
        app.session = Some(session);
        app.store = Some(store);
        app.capture_log = Some(capture);
        app.controller = Some(receiver);
        app.mode = Mode::Idle;
        (directory, app, sender)
    }

    fn feed_app(app: &mut crate::app::App, sender: &UdpSocket, bytes: &[u8]) {
        let socket = &app.controller.as_ref().unwrap().socket;
        sender.send_to(bytes, socket.local_addr().unwrap()).unwrap();
        socket.set_nonblocking(false).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        socket.peek_from(&mut [0; 2048]).unwrap();
        socket.set_nonblocking(true).unwrap();
        app.poll_controller();
        app.persist();
        assert!(app.save_error.is_none(), "{:?}", app.save_error);
    }

    #[test]
    fn wire_capture_persistence_and_rebuild_preserve_user_edits() {
        use crate::{capture::CaptureLog, model::MatchType};
        let (_directory, mut app, sender) = capture_app(MatchType::Official);
        let capture_path = app.store.as_ref().unwrap().capture_path();
        let feed = |app: &mut crate::app::App, bytes: &[u8]| feed_app(app, &sender, bytes);
        let mut bytes = packet(0);
        bytes[9] = 0;
        bytes[10] = 0;
        bytes[11] = 0;
        bytes[12] = 1;
        bytes[22] = 0;
        bytes[92] = 0;
        feed(&mut app, &bytes);
        bytes[5] = 1;
        bytes[14..16].copy_from_slice(&599_i16.to_le_bytes());
        feed(&mut app, &bytes);
        bytes[5] = 2;
        bytes[28] = 5;
        feed(&mut app, &bytes);
        let mut other_feed = bytes;
        other_feed[..4].copy_from_slice(b"RGTD");
        feed(&mut app, &other_feed);
        bytes[5] = 3;
        bytes[10] = 3;
        feed(&mut app, &bytes);
        bytes[5] = 4;
        bytes[11] = 5;
        bytes[13] = 5;
        feed(&mut app, &bytes);
        bytes[5] = 5;
        bytes[11] = 0;
        bytes[22] = 1;
        bytes[10] = 1;
        feed(&mut app, &bytes);
        bytes[5] = 6;
        bytes[10] = 4;
        bytes[12] = 0;
        feed(&mut app, &bytes);
        bytes[5] = 7;
        bytes[14..16].copy_from_slice(&0_i16.to_le_bytes());
        feed(&mut app, &bytes);
        let session = app.session.as_mut().unwrap();
        assert!(session.markdown().contains("Goal share: 50%"));
        assert!(session.markdown().contains("#1 1/1"));
        let packets =
            CaptureLog::read(&capture_path, session.capture_session.as_deref().unwrap()).unwrap();
        assert_eq!(packets.len(), 9);
        assert!(packets[1].accepted); // Clock-only frames are archived too.
        assert!(!packets[3].accepted); // Both feeds are retained, only one is applied.
        assert!(!packets[8].active);
        assert_eq!(packets[8].packet, bytes);
        assert_eq!(
            session
                .entries
                .iter()
                .filter_map(|entry| entry.capture_id)
                .collect::<Vec<_>>(),
            vec![1, 3, 5, 6, 7, 8]
        );
        session.entries[1].statistics = None;
        session.entries[1].text = "Human correction".into();
        session.entries[4].undone = true;
        let before = session.clone();
        let raw_before = std::fs::read(&capture_path).unwrap();
        assert_eq!(crate::rebuild::rebuild(session, &packets).unwrap(), 1);
        let rebuilt = session.clone();
        assert_eq!(crate::rebuild::rebuild(session, &packets).unwrap(), 0);
        assert_eq!(session, &rebuilt);
        assert!(session.entries[4].undone);
        let mut restored = session.clone();
        restored.entries[1].statistics = None;
        assert_eq!(restored, before);
        assert_eq!(std::fs::read(&capture_path).unwrap(), raw_before);
    }

    fn assert_capture_menu_end(match_type: crate::model::MatchType) {
        use crate::{capture::CaptureLog, model::MatchType};
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let (_directory, mut app, sender) = capture_app(match_type);
        let mut bytes = packet(0);
        bytes[9] = 0;
        bytes[10] = 3;
        bytes[11] = 0;
        bytes[12] = 0;
        bytes[22] = 0;
        bytes[92] = 0;
        if match_type == MatchType::Combined {
            bytes[10] = 4;
        }
        feed_app(&mut app, &sender, &bytes);
        app.key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE), wall_time())
            .unwrap();
        bytes[5] += 1;
        if match_type == MatchType::Combined {
            bytes[10] = 3;
            bytes[88] = 9;
            feed_app(&mut app, &sender, &bytes);
            bytes[5] += 1;
            assert_eq!(
                app.session.as_ref().unwrap().current_game(),
                MatchType::GoldenGoal
            );
        }
        bytes[10] = 4;
        feed_app(&mut app, &sender, &bytes);
        let end_id = app.last_capture_id.unwrap();
        bytes[5] += 1;
        bytes[28] = 5;
        feed_app(&mut app, &sender, &bytes);
        bytes[5] += 1;
        bytes[28] = 2;
        feed_app(&mut app, &sender, &bytes);
        assert_eq!(app.session.as_ref().unwrap().entries.len(), 1);
        let packets = CaptureLog::read(
            &app.store.as_ref().unwrap().capture_path(),
            app.session
                .as_ref()
                .unwrap()
                .capture_session
                .as_deref()
                .unwrap(),
        )
        .unwrap();
        for packet in &packets {
            assert!(packet.accepted);
            assert_eq!(packet.active, packet.id <= end_id, "packet {}", packet.id);
            assert_eq!(
                packet.game,
                Some(if match_type == MatchType::Combined && packet.id == 1 {
                    MatchType::GoldenGoal
                } else {
                    MatchType::Official
                }),
            );
        }
        app.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), wall_time())
            .unwrap();
        app.poll_controller();
        app.persist();
        assert!(app.save_error.is_none());
        let session = app.session.as_mut().unwrap();
        assert_eq!(session.current_game(), MatchType::Official);
        assert!(session.is_finished());
        assert_eq!(session.entries.last().unwrap().capture_id, Some(end_id));
        let original = session.clone();
        assert_eq!(crate::rebuild::rebuild(session, &packets).unwrap(), 0);
        assert_eq!(*session, original);
    }

    #[test]
    fn capture_menu_keeps_post_end_packets_inactive_across_polls() {
        assert_capture_menu_end(crate::model::MatchType::Official);
    }

    #[test]
    fn capture_menu_keeps_combined_handoff_and_end_context_across_polls() {
        assert_capture_menu_end(crate::model::MatchType::Combined);
    }

    #[test]
    fn capture_menu_end_before_first_packet_keeps_zero_anchor_and_blocks_rebuild() {
        use crate::{
            capture::CaptureLog,
            model::{EventKind, MatchType},
        };
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let (_directory, mut app, sender) = capture_app(MatchType::Official);
        assert_eq!(app.last_capture_id, None);
        app.key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE), wall_time())
            .unwrap();
        let mut bytes = packet(0);
        bytes[9] = 0;
        bytes[10] = 0;
        bytes[11] = 0;
        bytes[22] = 0;
        bytes[92] = 0;
        feed_app(&mut app, &sender, &bytes);
        bytes[5] = 1;
        bytes[28] = 5;
        feed_app(&mut app, &sender, &bytes);
        app.key(
            KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE),
            wall_time(),
        )
        .unwrap();
        app.poll_controller();
        app.persist();
        assert!(app.save_error.is_none());
        let session = app.session.as_mut().unwrap();
        assert_eq!(session.entries.len(), 1);
        let end = &session.entries[0];
        assert_eq!(end.kind, EventKind::GameEnd);
        assert_eq!(end.timestamp, wall_time());
        assert_eq!(end.capture_after, Some(0));
        let packets = CaptureLog::read(
            &app.store.as_ref().unwrap().capture_path(),
            session.capture_session.as_deref().unwrap(),
        )
        .unwrap();
        assert_eq!(packets.len(), 2);
        assert!(packets.iter().all(|packet| packet.active));
        let original = session.clone();
        assert_eq!(crate::rebuild::rebuild(session, &packets).unwrap(), 0);
        assert_eq!(*session, original);
        session.delete(0);
        assert_eq!(crate::rebuild::rebuild(session, &packets).unwrap(), 0);
        session.restore(0);
        assert_eq!(crate::rebuild::rebuild(session, &packets).unwrap(), 0);
        assert_eq!(*session, original);
    }

    #[test]
    fn quitting_drains_a_controller_packet_already_queued_on_the_socket() {
        use crate::{
            app::{App, Mode},
            model::{Metadata, Session},
        };
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let receiver = receiver();
        let destination = receiver.socket.local_addr().unwrap();
        let mut app = App::new(None).unwrap();
        app.session = Some(Session::new(Metadata {
            event: "test".into(),
            home: "HULKs".into(),
            away: "HTWK".into(),
            date: wall_time().date_naive(),
            label: "game".into(),
            robots: std::array::from_fn(|_| String::new()),
        }));
        app.mode = Mode::Actions {
            selected: 0,
            timestamp: wall_time(),
            message_budget: None,
            gc_seconds_remaining: None,
        };
        let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
        sender.send_to(&packet(0), destination).unwrap();
        receiver.socket.set_nonblocking(false).unwrap();
        receiver
            .socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        receiver.socket.peek_from(&mut [0; 2048]).unwrap();
        receiver.socket.set_nonblocking(true).unwrap();
        app.controller = Some(receiver);
        assert!(
            app.key(
                KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE),
                wall_time()
            )
            .unwrap()
        );
        let session = app.session.as_ref().unwrap();
        assert_eq!(session.entries.len(), 1);
        assert_eq!(session.score(), (3, 2));
        assert!(session.entries[0].from_controller);
        assert!(app.pending_controller.is_empty());
    }

    #[test]
    fn poll_limits_work_and_reports_unsupported_version_without_updates() {
        let mut receiver = receiver();
        let destination = receiver.socket.local_addr().unwrap();
        let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
        receiver.enable_capture(0);
        let mut old = packet(0);
        old[4] = 18;
        sender.send_to(&old[..118], destination).unwrap();
        assert!(poll_when_ready(&mut receiver).is_empty());
        assert!(receiver.notice.as_ref().unwrap().contains("version 18"));
        assert!(receiver.source.is_none());
        assert!(receiver.captures.is_empty());
        for sequence in 0..65 {
            sender.send_to(&packet(sequence), destination).unwrap();
        }
        assert_eq!(poll_when_ready(&mut receiver).len(), 1);
        assert!(receiver.latest.as_ref().unwrap().packet_number <= 63);
        assert!(receiver.captures.len() <= 64);
        while receiver.latest.as_ref().unwrap().packet_number < 64 {
            assert!(poll_when_ready(&mut receiver).is_empty());
        }
        assert_eq!(receiver.latest.as_ref().unwrap().packet_number, 64);
        assert_eq!(receiver.captures.len(), 65);
        assert_eq!(receiver.next_capture_id(), 65);
    }
}
