use super::*;
use crate::{model::MatchType, test_support::*};

fn feed(app: &mut crate::app::App, sender: &UdpSocket, bytes: &[u8]) {
    let receiver = app.controller.as_mut().unwrap();
    sender
        .send_to(bytes, receiver.socket.local_addr().unwrap())
        .unwrap();
    receiver.socket.set_nonblocking(false).unwrap();
    receiver
        .socket
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    receiver.socket.peek(&mut [0; 158]).unwrap();
    receiver.socket.set_nonblocking(true).unwrap();
    app.poll_controller();
}

#[test]
fn wire_decoder_validates_feeds_and_tracks_either_team_slot() {
    for &(team, _) in crate::teams::TEAMS {
        for away in [false, true] {
            let opponent = if team == 24 { 14 } else { 24 };
            let mut bytes = wire(1, opponent, 3, 2);
            bytes[18] = team;
            bytes[13] = team;
            if away {
                let (head, tail) = bytes[18..].split_at_mut(70);
                head.swap_with_slice(tail);
            }
            let parsed = decode_capture(&bytes, false, team).unwrap().unwrap();
            assert_eq!(
                (parsed.tracked_score, parsed.opponent_number),
                (2, opponent)
            );
            assert!(decode_capture(&bytes, true, team).unwrap().is_none());
            bytes[..4].copy_from_slice(b"RGTD");
            assert!(decode_capture(&bytes, true, team).unwrap().is_some());
            bytes[4] = 19;
            assert!(decode_capture(&bytes, true, team).is_err());
        }
    }
    assert!(decode_capture(&wire(1, 14, 0, 0)[..20], false, 24).is_err());
}

#[test]
fn combined_match_handover_imports_official_scores_and_capture_scope() {
    let (_dir, mut app) = saved(MatchType::Combined);
    let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
    let capture = app
        .store
        .as_ref()
        .unwrap()
        .prepare_capture(app.session.as_mut().unwrap())
        .unwrap();
    let mut receiver = Receiver::for_test();
    receiver.enable_capture(capture.next_id());
    app.capture_log = Some(capture);
    app.controller = Some(receiver);
    feed(&mut app, &sender, &wire(1, 14, 4, 1));
    assert!(app.session.as_ref().unwrap().is_finished());
    feed(&mut app, &sender, &wire(2, 14, 4, 1));
    assert_eq!(
        app.session.as_ref().unwrap().current_game(),
        MatchType::GoldenGoal
    );
    feed(&mut app, &sender, &wire(0, 14, 0, 0));
    let session = app.session.as_ref().unwrap();
    assert_eq!(session.current_game(), MatchType::Official);
    assert_eq!(session.official_opponent, Some(14));
    assert_eq!(session.score(), (0, 0));
    feed(&mut app, &sender, &wire(1, 14, 4, 1));
    assert!(!app.session.as_ref().unwrap().is_finished());
    feed(&mut app, &sender, &wire(1, 14, 3, 0));
    feed(&mut app, &sender, &wire(2, 14, 3, 1));
    assert_eq!(app.session.as_ref().unwrap().score(), (1, 0));
    assert_eq!(
        app.pending_capture.last().unwrap().game,
        Some(MatchType::Official)
    );
    app.persist();
    assert!(app.save_error.is_none());
    let path = app.store.as_ref().unwrap().session_path().to_owned();
    drop(app);
    let reopened = crate::app::App::new(Some(&path)).unwrap();
    assert_eq!(
        reopened.session.as_ref().unwrap().official_opponent,
        Some(14)
    );
    assert!(reopened.controller.is_none());
}

#[test]
fn discovery_requests_are_explicit_and_rate_limited() {
    let server = UdpSocket::bind("127.0.0.1:0").unwrap();
    server
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
    let mut probe = crate::discovery::Probe::for_test(vec![server.local_addr().unwrap()]);
    let now = Instant::now();
    probe.request(&sender, now);
    probe.request(&sender, now);
    let mut bytes = [0; 5];
    assert_eq!(server.recv_from(&mut bytes).unwrap().0, 5);
    assert_eq!(&bytes, b"RGTr\0");
    server.set_nonblocking(true).unwrap();
    assert_eq!(
        server.recv_from(&mut bytes).unwrap_err().kind(),
        io::ErrorKind::WouldBlock
    );
}

#[test]
fn source_pinning_retry_and_explicit_reconnect_do_not_mix_matches() {
    let mut receiver = Receiver::for_test();
    let old: SocketAddr = "127.0.0.1:12345".parse().unwrap();
    let new: SocketAddr = "127.0.0.1:12346".parse().unwrap();
    let server = UdpSocket::bind("127.0.0.1:0").unwrap();
    server
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    receiver.source = Some(old);
    receiver.true_data = true;
    receiver.request_destination = Some(server.local_addr().unwrap());
    let snapshot = decode_capture(&wire(1, 14, 3, 0), false, 24)
        .unwrap()
        .unwrap();
    assert!(
        receiver
            .accept(snapshot.clone(), old, Instant::now(), time())
            .is_some()
    );
    assert!(
        receiver
            .accept(snapshot.clone(), new, Instant::now(), time())
            .is_none()
    );
    receiver.last_true_receipt = None;
    receiver.request_true_data(Instant::now()).unwrap();
    let mut request = [0; 5];
    server.recv_from(&mut request).unwrap();
    assert_eq!(&request, b"RGTr\0");
    assert!(
        receiver
            .accept(snapshot.clone(), old, Instant::now(), time())
            .is_none()
    );
    assert!(
        receiver
            .accept(snapshot.clone(), new, Instant::now(), time())
            .is_some()
    );
    receiver.reconnect(14);
    let mut unrelated = snapshot.clone();
    unrelated.opponent_number = 5;
    assert!(
        receiver
            .accept(unrelated, new, Instant::now(), time())
            .is_none()
    );
    assert!(receiver.source.is_none());
    assert!(
        receiver
            .accept(snapshot, new, Instant::now(), time())
            .is_some()
    );
}
