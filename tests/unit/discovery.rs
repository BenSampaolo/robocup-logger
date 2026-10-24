use super::*;

#[test]
fn interface_addresses_produce_only_valid_broadcast_destinations() {
    let text =
        "inet 10.0.0.5 netmask 0xffffff00 broadcast 10.0.0.255\ninet 127.0.0.1\nbroadcast invalid";
    assert_eq!(
        interface_broadcasts(text),
        vec![Ipv4Addr::new(10, 0, 0, 255)]
    );
}
