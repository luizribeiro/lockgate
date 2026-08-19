use std::io::ErrorKind;
use std::net::{Ipv4Addr, SocketAddrV4, TcpStream};
use std::time::{SystemTime, UNIX_EPOCH};

wit_bindgen::generate!({
    path: "wit",
    world: "fixture",
});

struct Fixture;

impl exports::test::wasi::guest::Guest for Fixture {
    fn clock_seconds() -> u64 {
        println!("baseline WASI stdout is available");
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("the host wall clock must be after the Unix epoch")
            .as_secs()
    }

    fn network_denied() -> bool {
        let loopback = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 9);
        TcpStream::connect(loopback)
            .expect_err("the default WASI context must deny outbound sockets")
            .kind()
            == ErrorKind::PermissionDenied
    }
}

export!(Fixture);
