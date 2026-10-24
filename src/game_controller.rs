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
    pub tracked_score: u8,
    pub tracked_message_budget: u16,
    pub tracked_penalties: [u8; 20],
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
            crate::teams::display_name(self.kicking_team)
        };
        format!(
            "{phase} / {state} / {} half; set play: {set_play}; kicking team: {kicking_team}; stopped: {}; clock {}; secondary {}",
            if self.first_half { "first" } else { "second" },
            if self.stopped { "yes" } else { "no" },
            self.clock(),
            format_clock(self.secondary_time),
        )
    }

    pub(crate) fn starts_official_after(&self, golden_goal_opponent: u8) -> bool {
        self.opponent_number != golden_goal_opponent
            || (self.game_phase == 0
                && self.first_half
                && matches!(self.state, 0..=2)
                && self.tracked_score == 0
                && self.opponent_score == 0)
    }

    fn same_game_state(&self, other: &Self) -> bool {
        self.same_match_state(other) && self.tracked_penalties == other.tracked_penalties
    }

    pub(crate) fn same_match_state(&self, other: &Self) -> bool {
        self.game_phase == other.game_phase
            && self.state == other.state
            && self.set_play == other.set_play
            && self.first_half == other.first_half
            && self.stopped == other.stopped
            && self.kicking_team == other.kicking_team
            && self.tracked_score == other.tracked_score
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
pub(crate) fn decode_capture(
    packet: &[u8],
    true_data: bool,
    tracked_team: u8,
) -> Result<Option<Snapshot>, String> {
    parse_team_feed(packet, true_data, tracked_team)
}

fn parse_team_feed(
    packet: &[u8],
    true_data: bool,
    tracked_team: u8,
) -> Result<Option<Snapshot>, String> {
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

    let (tracked, opponent) = match (packet[18] == tracked_team, packet[88] == tracked_team) {
        (true, true) => {
            return Err("Ignored GameController packet: duplicate tracked team ID".to_owned());
        }
        (true, false) => (18, 88),
        (false, true) => (88, 18),
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
        tracked_score: packet[tracked + 4],
        tracked_message_budget: u16::from_le_bytes([packet[tracked + 8], packet[tracked + 9]]),
        tracked_penalties: std::array::from_fn(|player| packet[tracked + 10 + 3 * player]),
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

struct Detection {
    source: SocketAddr,
    teams: (u8, u8),
    received_at: Instant,
}

pub struct Receiver {
    socket: UdpSocket,
    probe: crate::discovery::Probe,
    detected: Option<Detection>,
    discovery_only: bool,
    source_from_request: bool,
    tracked_team: u8,
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
    pub allow_same_opponent_restart: Option<u8>,
    pub ignored_opponent: Option<u8>,
    acquisition_opponent: Option<u8>,
    pub awaiting_official_start: bool,
    pub latest: Option<Snapshot>,
    pub received_at: Option<Instant>,
    pub source: Option<SocketAddr>,
    pub notice: Option<String>,
}

impl Receiver {
    pub fn bind(tracked_team: u8, configured_opponent: Option<u8>) -> io::Result<Self> {
        if crate::teams::team_name(tracked_team).is_none()
            || configured_opponent.is_some_and(|opponent| {
                opponent == tracked_team || crate::teams::team_name(opponent).is_none()
            })
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Choose two distinct supported teams",
            ));
        }
        let socket = UdpSocket::bind("0.0.0.0:3838")?;
        socket.set_nonblocking(true)?;
        socket.set_broadcast(true)?;
        Ok(Self {
            socket,
            probe: crate::discovery::Probe::enabled(),
            detected: None,
            discovery_only: false,
            source_from_request: false,
            tracked_team,
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
            allow_same_opponent_restart: None,
            ignored_opponent: None,
            acquisition_opponent: None,
            awaiting_official_start: false,
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
            probe: crate::discovery::Probe::default(),
            detected: None,
            discovery_only: false,
            source_from_request: false,
            tracked_team: HULKS_TEAM_NUMBER,
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
            // Synthetic fixtures start without active discovery requests.
            auto_true_data: false,
            allow_opponent_change: false,
            allow_same_opponent_restart: None,
            ignored_opponent: None,
            acquisition_opponent: None,
            awaiting_official_start: false,
            latest: None,
            received_at: None,
            source: None,
            notice: None,
        }
    }

    pub(crate) fn bind_discovery() -> io::Result<Self> {
        let mut receiver = Self::bind(HULKS_TEAM_NUMBER, None)?;
        receiver.discovery_only = true;
        Ok(receiver)
    }

    pub(crate) fn detected_teams(&self) -> Option<(u8, u8)> {
        let detected = self.detected.as_ref()?;
        (detected.received_at.elapsed() <= STALE_AFTER
            && detected.teams.0 != detected.teams.1
            && crate::teams::team_name(detected.teams.0).is_some()
            && crate::teams::team_name(detected.teams.1).is_some())
        .then_some(detected.teams)
    }

    pub(crate) fn discovery_status(&self) -> (String, bool) {
        if let Some(detected) = &self.detected {
            let age = detected.received_at.elapsed();
            if age <= STALE_AFTER {
                return (
                    format!(
                        "GC reachable at {} • {} / {}",
                        detected.source.ip(),
                        crate::teams::display_name(detected.teams.0),
                        crate::teams::display_name(detected.teams.1)
                    ),
                    true,
                );
            }
            return (
                format!("GC no recent response • last seen {}", detected.source.ip()),
                false,
            );
        }
        if let Some(error) = &self.probe.error {
            return (format!("GC discovery request failed: {error}"), false);
        }
        (
            "GC searching: 10.0.0.10 + LAN • no response yet".into(),
            false,
        )
    }

    fn observe_detection(&mut self, bytes: &[u8], source: SocketAddr, now: Instant) {
        let Some(&first_team) = bytes.get(18) else {
            return;
        };
        let true_data = bytes.get(..4) == Some(b"RGTD");
        if (self.discovery_only || self.source.is_none_or(|pinned| pinned.ip() == source.ip()))
            && let Ok(Some(snapshot)) = parse_team_feed(bytes, true_data, first_team)
        {
            self.detected = Some(Detection {
                source,
                teams: (first_team, snapshot.opponent_number),
                received_at: now,
            });
        }
    }

    pub fn enable_capture(&mut self, next_id: u64) {
        self.next_capture_id = next_id;
        self.capture_enabled = true;
    }

    pub fn next_capture_id(&self) -> u64 {
        self.next_capture_id
    }

    pub(crate) fn opponent_number(&self) -> Option<u8> {
        self.opponent_number
    }

    pub(crate) fn reconnect(&mut self, opponent: u8) {
        // Keep the socket, capture IDs and history owner; only explicit action unpins IPs.
        self.acquisition_opponent = Some(opponent);
        self.opponent_number = None;
        self.source = None;
        self.source_from_request = false;
        self.true_source = None;
        self.retired_true_source = None;
        self.last_true_receipt = None;
        self.last_true_data_request = None;
        self.true_data = false;
        self.auto_true_data = true;
        self.detected = None;
        self.ignored_opponent = None;
        self.allow_opponent_change = false;
        self.allow_same_opponent_restart = None;
        self.awaiting_official_start = false;
        self.notice = None;
        self.rebaseline();
    }

    pub fn set_true_data(&mut self, enabled: bool) -> io::Result<()> {
        if enabled && self.source.is_none() {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "Wait for a GameController discovery response before selecting a feed",
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
            self.observe_detection(&buffer[..length], source, now);
            if self.discovery_only {
                continue;
            }
            // A monitor reply can establish the source without any RGme broadcast.
            if self.auto_true_data
                && self.source.is_none()
                && buffer[..length].get(..4) == Some(b"RGTD")
                && let Ok(Some(snapshot)) =
                    parse_team_feed(&buffer[..length], true, self.tracked_team)
                && self.ignored_opponent != Some(snapshot.opponent_number)
                && self
                    .acquisition_opponent
                    .is_none_or(|opponent| opponent == snapshot.opponent_number)
            {
                self.source = Some(source);
                self.source_from_request = true;
                self.set_true_data(true)?;
            }
            let true_data = self.true_data;
            let parsed = parse_team_feed(&buffer[..length], true_data, self.tracked_team);
            let (accepted, mut update) = match parsed {
                Ok(Some(snapshot)) => {
                    let handover = (
                        self.allow_opponent_change,
                        self.allow_same_opponent_restart,
                        self.awaiting_official_start,
                    );
                    let (accepted, update) = self.accept_packet(snapshot, source, now, received_at);
                    if update.is_some() && self.auto_true_data {
                        // Discover the controller, not its delayed match state/opponent.
                        self.set_true_data(true)?;
                        self.opponent_number = None;
                        (
                            self.allow_opponent_change,
                            self.allow_same_opponent_restart,
                            self.awaiting_official_start,
                        ) = handover;
                        (false, None)
                    } else {
                        (accepted, update)
                    }
                }
                Ok(None) => {
                    let header = if true_data { b"RGTD" } else { b"RGme" };
                    if self.latest.is_none()
                        && length == PACKET_SIZE
                        && buffer.get(..4) == Some(header)
                    {
                        self.notice = Some(format!(
                            "GC detected: {} vs {}; waiting for {}",
                            crate::teams::display_name(buffer[18]),
                            crate::teams::display_name(buffer[88]),
                            crate::teams::display_name(self.tracked_team)
                        ));
                    }
                    (false, None)
                }
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
        if (self.discovery_only || self.auto_true_data)
            && self.source.is_none()
            && (!self.discovery_only
                || self
                    .detected
                    .as_ref()
                    .is_none_or(|detected| detected.received_at.elapsed() > STALE_AFTER))
        {
            self.probe.request(&self.socket, Instant::now());
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
            || self
                .acquisition_opponent
                .is_some_and(|opponent| opponent != snapshot.opponent_number)
            || (self.true_data && self.retired_true_source == Some(source))
        {
            return (false, None);
        }
        let source_matches = if self.true_data {
            self.source.is_some_and(|pinned| pinned.ip() == source.ip())
                && self.true_source.is_none_or(|pinned| pinned == source)
        } else {
            self.source.is_none_or(|pinned| {
                if self.source_from_request {
                    pinned.ip() == source.ip()
                } else {
                    pinned == source
                }
            })
        };
        if !source_matches {
            self.notice = Some(format!(
                "Ignored GameController source {source}; Actions → Reconnect/switch GC to reacquire"
            ));
            return (false, None);
        }
        let opponent_changed = self
            .opponent_number
            .is_some_and(|pinned| pinned != snapshot.opponent_number);
        if opponent_changed && !self.allow_opponent_change {
            self.notice = Some(format!(
                "Ignored GameController opponent {}; reconnect only accepts the current match",
                crate::teams::display_name(snapshot.opponent_number)
            ));
            return (false, None);
        }
        let same_opponent_restart = self.allow_same_opponent_restart.is_some_and(|opponent| {
            snapshot.opponent_number == opponent && snapshot.starts_official_after(opponent)
        });
        // Do not import a late test end into a newly observed same-opponent pregame.
        // Reconnect explicitly clears this guard if Playing was never observed.
        if self.awaiting_official_start && snapshot.state == 4 {
            return (false, None);
        }
        // Positive match-reset evidence, not a recreated UDP sender, permits a new sequence.
        let mut previous = if opponent_changed
            || same_opponent_restart
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
            self.source_from_request = false;
        }
        // A reopened receiver may attach directly to the new game. Pin that first
        // opponent for the rest of this batch as well, until the app imports it.
        if opponent_changed || same_opponent_restart || self.opponent_number.is_none() {
            self.allow_opponent_change = false;
            self.allow_same_opponent_restart = None;
        }
        if same_opponent_restart {
            self.awaiting_official_start = true;
        } else if snapshot.state == 3 {
            self.awaiting_official_start = false;
        }
        self.opponent_number = Some(snapshot.opponent_number);
        if !self.auto_true_data {
            self.acquisition_opponent = None;
        }
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
#[path = "../tests/unit/controller.rs"]
mod tests;
