mod common;

use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::thread;
use std::time::Duration;

use lockgate::{Host, HostBuilder, InvocationCtx, PluginConfig, PluginHandle, RuntimeLimits};

lockgate::host_bindings!({
    path: "tests/fixtures/http-client-guest/wit",
    world: "fixture",
});

use guest::HostExt as _;

const RESPONSE_BODY: &str = "lockgate-http works!";
const SHORT_TIMEOUT_MILLIS: u64 = 100;

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

fn serve_after_delay(delay: Duration) -> (String, thread::JoinHandle<SocketAddr>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (mut connection, peer) = listener.accept().unwrap();
        connection
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut request = [0; 1024];
        let _ = connection.read(&mut request).unwrap();
        thread::sleep(delay);
        let _ = write!(
            connection,
            "HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
        );
        peer
    });
    (format!("http://{address}"), server)
}

async fn admitted_client(origin: String) -> (Host<()>, PluginHandle) {
    let mut builder = HostBuilder::new(()).unwrap();
    let prepared = builder
        .prepare(
            "http-client",
            &common::HTTP_CLIENT_FIXTURE,
            PluginConfig {
                settings: Some(serde_json::json!({ "origin": origin })),
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
            InvocationCtx::bounded(common::INVOCATION_FUEL, common::INVOCATION_DEADLINE),
        )
        .await
        .unwrap();
    (builder.finish(), plugin)
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
    let (host, plugin) = admitted_client(allowed_origin.clone()).await;
    let guest = host.guest(&plugin).unwrap();

    let allowed_url = format!("{allowed_origin}/client");
    let response = guest
        .get(
            InvocationCtx::bounded(common::INVOCATION_FUEL, common::INVOCATION_DEADLINE),
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
            InvocationCtx::bounded(common::INVOCATION_FUEL, common::INVOCATION_DEADLINE),
            &blocked_url,
        )
        .await
        .unwrap()
        .unwrap_err();
    assert!(error.contains("HTTP request failed"), "{error}");
    assert_no_connection(blocked);
    host.shutdown().await;
}

#[tokio::test(flavor = "current_thread")]
async fn base_url_setting_resolves_to_its_origin() {
    let (allowed_origin, server) = serve_once();
    let base_url = format!("{allowed_origin}/v1");
    let (host, plugin) = admitted_client(base_url).await;
    let guest = host.guest(&plugin).unwrap();

    let allowed_url = format!("{allowed_origin}/client");
    let response = guest
        .get(
            InvocationCtx::bounded(common::INVOCATION_FUEL, common::INVOCATION_DEADLINE),
            &allowed_url,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(response.status, 201);
    assert_eq!(response.body, RESPONSE_BODY);
    server.join().unwrap();
    host.shutdown().await;
}

#[tokio::test(flavor = "current_thread")]
async fn first_byte_timeout_fails_a_stalled_request() {
    let (allowed_origin, server) = serve_after_delay(Duration::from_millis(500));
    let (host, plugin) = admitted_client(allowed_origin.clone()).await;
    let guest = host.guest(&plugin).unwrap();

    let error = guest
        .get_with_first_byte_timeout(
            InvocationCtx::bounded(common::INVOCATION_FUEL, common::INVOCATION_DEADLINE),
            &format!("{allowed_origin}/stall"),
            SHORT_TIMEOUT_MILLIS,
        )
        .await
        .unwrap()
        .unwrap_err();

    assert!(error.contains("HTTP request failed"), "{error}");
    assert!(error.to_ascii_lowercase().contains("timeout"), "{error}");
    server.join().unwrap();
    host.shutdown().await;
}

#[tokio::test(flavor = "current_thread")]
async fn normal_request_succeeds_with_generous_timeouts() {
    let (allowed_origin, server) = serve_once();
    let (host, plugin) = admitted_client(allowed_origin.clone()).await;
    let guest = host.guest(&plugin).unwrap();

    let response = guest
        .get_with_timeouts(
            InvocationCtx::bounded(common::INVOCATION_FUEL, common::INVOCATION_DEADLINE),
            &format!("{allowed_origin}/timeouts"),
            5_000,
        )
        .await
        .unwrap()
        .unwrap();

    assert_eq!(response.status, 201);
    assert_eq!(response.body, RESPONSE_BODY);
    server.join().unwrap();
    host.shutdown().await;
}
