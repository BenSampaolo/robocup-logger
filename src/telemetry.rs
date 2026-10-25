use chrono::{DateTime, FixedOffset, Local};
use std::{
    io,
    net::{IpAddr, SocketAddr, UdpSocket},
    time::{Duration, Instant},
};

const STALE_AFTER: Duration = Duration::from_secs(5);

pub(crate) struct Robot {
    pub x: f64,
    pub y: f64,
    pub heading: f64,
    pub ball: Option<(f64, f64)>,
    pub ball_age: f64,
    pub fallen: bool,
    pub received: Instant,
}

impl Robot {
    pub fn fresh(&self) -> bool {
        self.received.elapsed() <= STALE_AFTER
    }
}

#[derive(Default)]
pub(crate) struct Telemetry {
    pub robots: [Option<Robot>; 5],
    capture_paused: bool,
    socket: Option<UdpSocket>,
    identity: Option<(IpAddr, u8)>,
    last_bind: Option<Instant>,
    error: Option<String>,
}

impl Telemetry {
    pub fn set_capture_paused(&mut self, paused: bool) {
        if self.capture_paused == paused {
            return;
        }
        self.capture_paused = paused;
        self.socket = None;
        self.last_bind = None;
        self.error = None;
        self.robots = Default::default();
    }

    pub fn poll(
        &mut self,
        source: Option<IpAddr>,
        team: u8,
        mut record: impl FnMut(DateTime<FixedOffset>, SocketAddr, &[u8]),
    ) {
        let identity = source.map(|source| (source, team));
        if self.identity != identity {
            *self = Self {
                identity,
                capture_paused: self.capture_paused,
                ..Self::default()
            };
        }
        if self.capture_paused {
            return;
        }
        let Some((source, team)) = self.identity else {
            return;
        };
        let now = Instant::now();
        if self.socket.is_none() {
            if self
                .last_bind
                .is_some_and(|last| now.duration_since(last) < Duration::from_secs(2))
            {
                return;
            }
            self.last_bind = Some(now);
            let address = "0.0.0.0:3940";
            match UdpSocket::bind(address).and_then(|socket| {
                socket.set_nonblocking(true)?;
                Ok(socket)
            }) {
                Ok(socket) => {
                    self.socket = Some(socket);
                    self.error = None;
                }
                Err(error) => {
                    self.error = Some(format!("Robot UDP 3940: {error}"));
                    return;
                }
            }
        }
        let mut bytes = [0; 65_535];
        for _ in 0..256 {
            match self.socket.as_ref().unwrap().recv_from(&mut bytes) {
                Ok((length, sender)) if sender.ip() == source => {
                    record(Local::now().fixed_offset(), sender, &bytes[..length]);
                    match decode(&bytes[..length], team, Instant::now()) {
                        Ok(Some((player, robot))) => {
                            self.robots[player] = Some(robot);
                            self.error = None;
                        }
                        Ok(None) => {}
                        Err(error) => self.error = Some(error.into()),
                    }
                }
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) => {
                    self.error = Some(format!("Robot UDP 3940: {error}"));
                    self.socket = None;
                    break;
                }
            }
        }
    }

    pub fn status(&self) -> String {
        if self.capture_paused {
            return "Robot capture paused — retry saving".into();
        }
        if let Some(error) = &self.error {
            return error.clone();
        }
        if self.identity.is_none() {
            return "Waiting for GC".into();
        }
        let fresh = self
            .robots
            .iter()
            .flatten()
            .filter(|robot| robot.fresh())
            .count();
        format!("{fresh}/5")
    }
}

pub(crate) fn decode(
    bytes: &[u8],
    team: u8,
    received: Instant,
) -> Result<Option<(usize, Robot)>, &'static str> {
    if bytes.len() != 36 || &bytes[4..8] != b"RGrt" {
        return Err("Invalid forwarded robot status");
    }
    let packet = &bytes[4..];
    if packet[4] != 4 {
        return Err("Unsupported robot status version (expected RGrt v4)");
    }
    if packet[6] != team || !(1..=5).contains(&packet[5]) {
        return Ok(None);
    }
    if packet[7] > 1 {
        return Err("Invalid robot fallen state");
    }
    let number = |offset: usize| {
        f64::from(f32::from_le_bytes(
            packet[offset..offset + 4].try_into().unwrap(),
        ))
    };
    let (x, y, heading) = (number(8) / 1000.0, number(12) / 1000.0, number(16));
    if ![x, y, heading].iter().all(|value| value.is_finite()) {
        return Err("Invalid robot pose");
    }
    let ball_age = number(20);
    let ball = if ball_age == -1.0 {
        None
    } else {
        let (bx, by) = (number(24) / 1000.0, number(28) / 1000.0);
        if ball_age < 0.0 || ![ball_age, bx, by].iter().all(|value| value.is_finite()) {
            return Err("Invalid robot ball estimate");
        }
        let (sin, cos) = heading.sin_cos();
        Some((x + bx * cos - by * sin, y + bx * sin + by * cos))
    };
    Ok(Some((
        usize::from(packet[5] - 1),
        Robot {
            x,
            y,
            heading,
            ball,
            ball_age,
            fallen: packet[7] == 1,
            received,
        },
    )))
}
