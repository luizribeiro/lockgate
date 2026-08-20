mod common;

use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::thread;
use std::time::Duration;

use lockgate::{HostBuilder, InvocationCtx, PluginConfig, RuntimeLimits};

lockgate::host_bindings!({
    path: "tests/fixtures/http-client-guest/wit",
    world: "fixture",
});

use guest::HostExt as _;

const RESPONSE_BODY: &str = "lockgate-http works!";

fn serve_once() -> (String, thread::JoinHandle<SocketAddr>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (mut connection, peer) = listener.accept().unwrap();
        connection
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut request = [0; 1024];
        let _ = connection.read(&mut request).unwrap();
        write!(
            connection,
            "HTTP/1.1 201 Created\r\ncontent-type: text/plain\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{RESPONSE_BODY}",
            RESPONSE_BODY.len(),
        )
        .unwrap();
        peer
    });
    (format!("http://{address}"), server)
}

fn assert_no_connection(listener: TcpListener) {
    listener.set_nonblocking(true).unwrap();
    thread::sleep(Duration::from_millis(50));
    match listener.accept() {
        Err(error) if error.kind() == ErrorKind::WouldBlock => {}
        Err(error) => panic!("unexpected accept error: {error}"),
        Ok((_, peer)) => panic!("request unexpectedly connected from {peer}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn guest_http_client_reaches_only_its_granted_origin() {
    let (allowed_origin, server) = serve_once();
    let mut builder = HostBuilder::new(()).unwrap();
    let prepared = builder
        .prepare(
            "http-client",
            &common::HTTP_CLIENT_FIXTURE,
            PluginConfig {
                settings: Some(serde_json::json!({ "origin": allowed_origin })),
                ..PluginConfig::default()
            },
        )
        .await
        .unwrap();
    let acceptance = prepared.accept_all();
    let plugin = builder
        .admit(
            prepared,
            acceptance,
            RuntimeLimits::default(),
            InvocationCtx::bounded(common::INVOCATION_FUEL),
        )
        .await
        .unwrap();
    let host = builder.finish();
    let guest = host.guest(&plugin).unwrap();

    let allowed_url = format!("{allowed_origin}/client");
    let response = guest
        .get(
            InvocationCtx::bounded(common::INVOCATION_FUEL),
            &allowed_url,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(response.status, 201);
    assert_eq!(response.body, RESPONSE_BODY);
    server.join().unwrap();

    let blocked = TcpListener::bind("127.0.0.1:0").unwrap();
    let blocked_url = format!("http://{}/blocked", blocked.local_addr().unwrap());
    let error = guest
        .get(
            InvocationCtx::bounded(common::INVOCATION_FUEL),
            &blocked_url,
        )
        .await
        .unwrap()
        .unwrap_err();
    assert!(error.contains("HTTP request failed"), "{error}");
    assert_no_connection(blocked);
}
