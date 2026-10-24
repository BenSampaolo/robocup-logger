use super::*;
use crate::test_support::packet;

#[test]
fn failed_capture_batches_roll_back_and_reject_wrong_ownership() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("capture.jsonl");
    let mut log = CaptureLog::open(&path, "owner").unwrap();
    let original = fs::read(&path).unwrap();
    assert!(
        log.append_with(&[packet(1, 0, 0)], |file, bytes| {
            file.write_all(&bytes[..bytes.len() / 2])?;
            Err(io::Error::other("simulated disk error"))
        })
        .is_err()
    );
    assert_eq!(fs::read(&path).unwrap(), original);
    assert_eq!(log.next_id(), 1);
    log.append(&[packet(1, 0, 0)]).unwrap();
    drop(log);
    assert_eq!(CaptureLog::open(&path, "owner").unwrap().next_id(), 2);
    assert!(CaptureLog::open(&path, "different-owner").is_err());
    OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(b"{")
        .unwrap();
    assert!(CaptureLog::open(&path, "owner").is_err());
}
