use std::{
    collections::BTreeMap,
    io,
    net::{IpAddr, SocketAddr},
    time::Instant,
};

use chrono::{DateTime, FixedOffset, TimeDelta};

use crate::{
    capture::{CaptureLog, RobotCaptureLog},
    game_controller::{self, Snapshot},
    model::{EventKind, MatchType, Session},
    storage::{Store, regular_file_exists},
    telemetry,
};

const FRESH_FOR: f64 = 5.0;

pub(crate) struct Timeline {
    pub start: DateTime<FixedOffset>,
    pub duration: f64,
    pub notes: Vec<(f64, usize)>,
    pub events: Vec<(f64, usize)>,
    pub controllers: Vec<ControllerSample>,
    pub warning: Option<String>,
    entry_times: Vec<f64>,
    contexts: Vec<Context>,
    poses: BTreeMap<(u8, u8), Vec<PoseSample>>,
}

pub struct ControllerSample {
    pub time: f64,
    pub clock_started: f64,
    pub snapshot: Snapshot,
    pub continuous: bool,
    pub source: IpAddr,
}

#[derive(Clone, Copy)]
pub struct Pose {
    pub x: f64,
    pub y: f64,
    pub heading: f64,
    pub ball: Option<(f64, f64)>,
    pub ball_age: f64,
    pub fallen: bool,
    pub age: f64,
}

struct RecordedController {
    received_at: DateTime<FixedOffset>,
    snapshot: Snapshot,
    source: SocketAddr,
    true_data: bool,
    game: Option<MatchType>,
    active: bool,
    baseline: bool,
    order: usize,
}

struct RecordedPose {
    received_at: DateTime<FixedOffset>,
    source: IpAddr,
    team: u8,
    player: u8,
    pose: Pose,
}

struct Context {
    time: f64,
    source: IpAddr,
    opponent: Option<u8>,
    active: bool,
    epoch: usize,
}

struct PoseSample {
    time: f64,
    epoch: usize,
    pose: Pose,
}

impl Timeline {
    pub fn load(store: &Store, session: &Session) -> io::Result<Self> {
        session.validate().map_err(invalid_data)?;
        let gc_path = store.capture_path();
        let robot_path = store.robot_capture_path();
        let has_gc = regular_file_exists(&gc_path)?;
        let has_robots = regular_file_exists(&robot_path)?;
        let owner = session.capture_session.as_deref();
        if (has_gc || has_robots) && owner.is_none() {
            return Err(invalid_data("Capture exists without a session owner"));
        }
        let references = session
            .entries
            .iter()
            .flat_map(|entry| [entry.capture_id, entry.capture_after])
            .flatten();
        let last_reference = references.max();
        if !has_gc && last_reference.is_some() {
            return Err(invalid_data(
                "GC capture is missing but session entries reference it",
            ));
        }
        let tracked = session.metadata.tracked_team_number();
        let mut bounds: Option<(DateTime<FixedOffset>, DateTime<FixedOffset>)> = None;
        let mut observe_time = |time| match &mut bounds {
            Some((start, end)) => {
                *start = (*start).min(time);
                *end = (*end).max(time);
            }
            None => bounds = Some((time, time)),
        };
        for entry in &session.entries {
            observe_time(entry.timestamp);
        }
        let mut recorded_controllers = Vec::new();
        let mut last_id = 0;
        let mut decode_error = None;
        if has_gc {
            CaptureLog::visit(&gc_path, owner.unwrap(), |packet| {
                last_id = packet.id;
                observe_time(packet.received_at);
                if !packet.accepted {
                    return;
                }
                match game_controller::decode_capture(&packet.packet, packet.true_data, tracked) {
                    Ok(Some(snapshot)) => recorded_controllers.push(RecordedController {
                        received_at: packet.received_at,
                        snapshot,
                        source: packet.source,
                        true_data: packet.true_data,
                        game: packet.game,
                        active: packet.active,
                        baseline: packet.baseline,
                        order: recorded_controllers.len(),
                    }),
                    _ => {
                        decode_error.get_or_insert_with(|| {
                            invalid_data(format!(
                                "Accepted GC packet {} cannot be decoded for the tracked team",
                                packet.id
                            ))
                        });
                    }
                }
            })?;
        }
        if let Some(error) = decode_error {
            return Err(error);
        }
        if last_reference.is_some_and(|id| id > last_id)
            || session
                .entries
                .iter()
                .any(|entry| entry.capture_id == Some(0))
        {
            return Err(invalid_data(
                "Session references packets absent from the GC capture",
            ));
        }
        let mut recorded_poses = Vec::new();
        let decode_clock = Instant::now();
        if has_robots {
            RobotCaptureLog::visit(&robot_path, owner.unwrap(), |packet| {
                observe_time(packet.received_at);
                let Some(&team) = packet.packet.get(10) else {
                    return;
                };
                let Ok(Some((player, robot))) =
                    telemetry::decode(&packet.packet, team, decode_clock)
                else {
                    return;
                };
                recorded_poses.push(RecordedPose {
                    received_at: packet.received_at,
                    source: packet.source.ip(),
                    team,
                    player: player as u8 + 1,
                    pose: Pose {
                        x: robot.x,
                        y: robot.y,
                        heading: robot.heading,
                        ball: robot.ball,
                        ball_age: robot.ball_age,
                        fallen: robot.fallen,
                        age: 0.0,
                    },
                });
            })?;
        }
        let (start, end) = bounds.unwrap_or_else(|| {
            let midnight = session
                .metadata
                .date
                .and_hms_opt(0, 0, 0)
                .unwrap()
                .and_utc()
                .fixed_offset();
            (midnight, midnight)
        });
        let mut timeline = Self {
            start,
            duration: seconds_since(end, start),
            notes: Vec::new(),
            events: Vec::new(),
            controllers: Vec::new(),
            warning: None,
            entry_times: session
                .entries
                .iter()
                .map(|entry| seconds_since(entry.timestamp, start))
                .collect(),
            contexts: Vec::new(),
            poses: BTreeMap::new(),
        };
        for (index, entry) in session
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| !entry.undone)
        {
            let time = timeline.entry_times[index];
            timeline.events.push((time, index));
            if entry.kind == EventKind::Note
                || (entry.kind == EventKind::Custom && !entry.from_controller)
                || (!entry.text.trim().is_empty() && !entry.has_controller_snapshot())
            {
                timeline.notes.push((time, index));
            }
        }
        timeline.events.sort_by(|a, b| a.0.total_cmp(&b.0));
        timeline.notes.sort_by(|a, b| a.0.total_cmp(&b.0));
        recorded_controllers.sort_by_key(|sample| sample.received_at);
        let mut epoch = 0;
        for (index, sample) in recorded_controllers.iter().enumerate() {
            let time = seconds_since(sample.received_at, start);
            let continuous = index.checked_sub(1).is_some_and(|previous| {
                let previous = &recorded_controllers[previous];
                sample.active
                    && previous.active
                    && !sample.baseline
                    && sample.order == previous.order + 1
                    && seconds_since(sample.received_at, previous.received_at) <= FRESH_FOR
                    && sample.source == previous.source
                    && sample.true_data == previous.true_data
                    && sample.game == previous.game
                    && sample.snapshot.opponent_number == previous.snapshot.opponent_number
                    && sample
                        .snapshot
                        .packet_number
                        .wrapping_sub(previous.snapshot.packet_number)
                        == 1
            });
            let changed_frame = index.checked_sub(1).is_some_and(|previous| {
                let previous = &recorded_controllers[previous];
                sample.snapshot.first_half != previous.snapshot.first_half
                    || sample.snapshot.game_phase != previous.snapshot.game_phase
            });
            if !continuous || changed_frame {
                epoch += 1;
            }
            timeline.contexts.push(Context {
                time,
                source: sample.source.ip(),
                opponent: Some(sample.snapshot.opponent_number),
                active: sample.active,
                epoch,
            });
            if sample.active {
                let clock_started = timeline
                    .controllers
                    .last()
                    .filter(|previous| {
                        continuous
                            && previous.snapshot.seconds_remaining
                                == sample.snapshot.seconds_remaining
                            && previous.snapshot.state == sample.snapshot.state
                            && previous.snapshot.stopped == sample.snapshot.stopped
                            && previous.snapshot.game_phase == sample.snapshot.game_phase
                            && previous.snapshot.first_half == sample.snapshot.first_half
                    })
                    .map_or(time, |previous| previous.clock_started);
                timeline.controllers.push(ControllerSample {
                    time,
                    clock_started,
                    snapshot: sample.snapshot.clone(),
                    continuous,
                    source: sample.source.ip(),
                });
            }
        }
        let metadata_only = recorded_controllers.is_empty();
        recorded_poses.sort_by_key(|sample| sample.received_at);
        for sample in recorded_poses {
            let time = seconds_since(sample.received_at, start);
            if metadata_only {
                let opponent = crate::teams::team_number(session.metadata.opponent_name());
                if sample.team != tracked && Some(sample.team) != opponent {
                    continue;
                }
                if timeline
                    .contexts
                    .last()
                    .is_none_or(|context| context.source != sample.source)
                {
                    epoch += 1;
                    timeline.contexts.push(Context {
                        time,
                        source: sample.source,
                        opponent,
                        active: true,
                        epoch,
                    });
                }
            }
            let Some(context) = timeline.context(time) else {
                continue;
            };
            if !context.active
                || context.source != sample.source
                || (sample.team != tracked && Some(sample.team) != context.opponent)
            {
                continue;
            }
            let epoch = context.epoch;
            timeline
                .poses
                .entry((sample.team, sample.player))
                .or_default()
                .push(PoseSample {
                    time,
                    epoch,
                    pose: sample.pose,
                });
        }
        let mut warnings = Vec::new();
        if !has_gc {
            warnings.push("GC archive missing; clock and half unavailable");
        } else if timeline.controllers.is_empty() {
            warnings.push("No accepted active GC samples; clock and half unavailable");
        }
        if !has_robots {
            warnings.push("Robot archive missing; positions unavailable");
        } else if timeline.poses.is_empty() {
            warnings.push("No matching valid robot samples; positions unavailable");
        } else if metadata_only {
            warnings
                .push("Partial robot replay uses metadata team IDs; GC source and half unverified");
        }
        if !warnings.is_empty() {
            timeline.warning = Some(warnings.join(" · "));
        }
        Ok(timeline)
    }

    pub fn controller(&self, time: f64) -> Option<&ControllerSample> {
        let index = self
            .controllers
            .partition_point(|sample| sample.time <= time);
        index.checked_sub(1).map(|index| &self.controllers[index])
    }

    pub fn robots(&self, time: f64) -> Vec<(u8, u8, Pose)> {
        let Some(context) = self.context(time).filter(|context| context.active) else {
            return Vec::new();
        };
        self.poses
            .iter()
            .filter_map(|(&(team, player), samples)| {
                let index = samples
                    .partition_point(|sample| sample.time <= time)
                    .checked_sub(1)?;
                let sample = &samples[index];
                let age = time - sample.time;
                if sample.epoch != context.epoch || !(0.0..=FRESH_FOR).contains(&age) {
                    return None;
                }
                let mut pose = sample.pose;
                pose.age = age;
                if pose.ball.is_some() {
                    pose.ball_age += age;
                }
                Some((team, player, pose))
            })
            .collect()
    }

    pub fn wall_time(&self, time: f64) -> DateTime<FixedOffset> {
        let time = if time.is_finite() {
            time.clamp(0.0, self.duration)
        } else {
            0.0
        };
        let elapsed = TimeDelta::seconds(time.trunc() as i64)
            + TimeDelta::nanoseconds((time.fract() * 1_000_000_000.0).round() as i64);
        self.start.checked_add_signed(elapsed).unwrap_or(self.start)
    }

    pub fn entry_time(&self, index: usize) -> Option<f64> {
        self.entry_times.get(index).copied()
    }

    fn context(&self, time: f64) -> Option<&Context> {
        let index = self
            .contexts
            .partition_point(|context| context.time <= time);
        index.checked_sub(1).map(|index| &self.contexts[index])
    }
}

fn seconds_since(time: DateTime<FixedOffset>, start: DateTime<FixedOffset>) -> f64 {
    let delta = time.signed_duration_since(start);
    delta.num_seconds() as f64 + f64::from(delta.subsec_nanos()) / 1_000_000_000.0
}

fn invalid_data(message: impl ToString) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_string())
}
