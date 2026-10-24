use std::{
    net::{Ipv4Addr, SocketAddr, UdpSocket},
    time::{Duration, Instant},
};

use crate::game_controller::Receiver;

const RETRY_AFTER: Duration = Duration::from_secs(2);

#[derive(Default)]
pub(crate) struct Discovery {
    receiver: Option<Receiver>,
    last_bind: Option<Instant>,
    error: Option<String>,
}

impl Discovery {
    pub fn poll(&mut self) {
        let now = Instant::now();
        if self.receiver.is_none()
            && self
                .last_bind
                .is_none_or(|last| now.duration_since(last) >= RETRY_AFTER)
        {
            self.last_bind = Some(now);
            match Receiver::bind_discovery() {
                Ok(receiver) => {
                    self.receiver = Some(receiver);
                    self.error = None;
                }
                Err(error) => {
                    self.error = Some(format!("GC detection: cannot listen on UDP 3838: {error}"))
                }
            }
        }
        if let Some(receiver) = &mut self.receiver
            && let Err(error) = receiver.poll()
        {
            self.error = Some(format!("GC detection socket error: {error}"));
            self.receiver = None;
            self.last_bind = Some(now);
        }
    }

    pub fn detected_teams(&self) -> Option<(u8, u8)> {
        self.receiver.as_ref().and_then(Receiver::detected_teams)
    }

    pub fn status(&self) -> (String, bool) {
        if let Some(error) = &self.error {
            return (error.clone(), false);
        }
        self.receiver.as_ref().map_or_else(
            || {
                (
                    "GC discovery: checking 10.0.0.10 and local network".into(),
                    false,
                )
            },
            Receiver::discovery_status,
        )
    }
}

#[derive(Default)]
pub(crate) struct Probe {
    enabled: bool,
    last_request: Option<Instant>,
    pub error: Option<String>,
    #[cfg(test)]
    destinations: Vec<SocketAddr>,
}

impl Probe {
    pub fn enabled() -> Self {
        Self {
            enabled: true,
            ..Self::default()
        }
    }

    pub fn request(&mut self, socket: &UdpSocket, now: Instant) {
        if !self.enabled
            || self
                .last_request
                .is_some_and(|last| now.duration_since(last) < RETRY_AFTER)
        {
            return;
        }
        self.last_request = Some(now);
        #[cfg(not(test))]
        let destinations = destinations();
        #[cfg(test)]
        let destinations = self.destinations.clone();
        let mut sent = false;
        let mut last_error = None;
        for destination in destinations {
            #[cfg(test)]
            assert!(
                destination.ip().is_loopback() && ![0, 3636, 3838].contains(&destination.port())
            );
            match socket.send_to(b"RGTr\0", destination) {
                Ok(_) => sent = true,
                Err(error) => last_error = Some(error.to_string()),
            }
        }
        self.error = if sent { None } else { last_error };
    }

    #[cfg(test)]
    pub fn for_test(destinations: Vec<SocketAddr>) -> Self {
        Self {
            destinations,
            ..Self::enabled()
        }
    }
}

#[cfg(not(test))]
fn destinations() -> Vec<SocketAddr> {
    let mut addresses = vec![Ipv4Addr::new(10, 0, 0, 10), Ipv4Addr::BROADCAST];
    // Directed broadcasts reach attached LANs even when a VPN owns the default route.
    #[cfg(target_os = "macos")]
    if let Ok(output) = std::process::Command::new("/sbin/ifconfig")
        .arg("-a")
        .output()
        && output.status.success()
    {
        addresses.extend(interface_broadcasts(&String::from_utf8_lossy(
            &output.stdout,
        )));
    }
    let mut result = Vec::new();
    for address in addresses {
        let destination = SocketAddr::from((address, 3636));
        if !result.contains(&destination) {
            result.push(destination);
        }
    }
    result
}

#[cfg(any(target_os = "macos", test))]
fn interface_broadcasts(text: &str) -> Vec<Ipv4Addr> {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .windows(2)
        .filter(|pair| pair[0] == "broadcast")
        .filter_map(|pair| pair[1].parse::<Ipv4Addr>().ok())
        .filter(|address| {
            !address.is_loopback() && !address.is_unspecified() && !address.is_multicast()
        })
        .collect()
}

#[cfg(test)]
#[path = "../tests/unit/discovery.rs"]
mod tests;
