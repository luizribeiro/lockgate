mod common;

use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use lockgate::{Host, HostBuilder, InvocationCtx, PluginConfig, PluginHandle, RuntimeLimits};
use rcgen::{BasicConstraints, CertificateParams, IsCa, Issuer, KeyPair, KeyUsagePurpose};
use rustls::{RootCertStore, ServerConfig, pki_types::PrivatePkcs8KeyDer};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio_rustls::TlsAcceptor;

lockgate::host_bindings!({
    path: "tests/fixtures/http-client-guest/wit",
    world: "fixture",
});

use guest::HostExt as _;

const RESPONSE_BODY: &str = "lockgate-http works!";
const SHORT_TIMEOUT_MILLIS: u64 = 100;

struct TlsServer {
    origin: String,
    roots: RootCertStore,
    task: tokio::task::JoinHandle<Result<SocketAddr, std::io::Error>>,
}

async fn serve_tls_once() -> TlsServer {
    let _ = rustls::crypto::ring::default_provider().install_default();

    let mut ca_params = CertificateParams::new(Vec::new()).unwrap();
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params.key_usages = vec![
        KeyUsagePurpose::DigitalSignature,
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::CrlSign,
    ];
    let ca_key = KeyPair::generate().unwrap();
    let ca_cert = ca_params.self_signed(&ca_key).unwrap();
    let issuer = Issuer::new(ca_params, ca_key);

    let leaf_params = CertificateParams::new(vec!["127.0.0.1".to_owned()]).unwrap();
    let leaf_key = KeyPair::generate().unwrap();
    let leaf_cert = leaf_params.signed_by(&leaf_key, &issuer).unwrap();
    let private_key = PrivatePkcs8KeyDer::from(leaf_key.serialize_der()).into();
    let server_config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![leaf_cert.der().clone()], private_key)
        .unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let acceptor = TlsAcceptor::from(Arc::new(server_config));
    let task = tokio::spawn(async move {
        let (stream, peer) = listener.accept().await?;
        let mut connection = acceptor.accept(stream).await?;
        let mut request = [0; 1024];
        let _ = connection.read(&mut request).await?;
        connection
            .write_all(
                format!(
                    "HTTP/1.1 201 Created\r\ncontent-type: text/plain\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{RESPONSE_BODY}",
                    RESPONSE_BODY.len(),
                )
                .as_bytes(),
            )
            .await?;
        connection.shutdown().await?;
        Ok(peer)
    });

    let mut roots = RootCertStore::empty();
    roots.add(ca_cert.der().clone()).unwrap();
    TlsServer {
        origin: format!("https://{address}"),
        roots,
        task,
    }
}

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
    admitted_client_with_options(origin, RuntimeLimits::default(), None).await
}

async fn admitted_client_with_limits(
    origin: String,
    limits: RuntimeLimits,
) -> (Host<()>, PluginHandle) {
    admitted_client_with_options(origin, limits, None).await
}

async fn admitted_client_with_options(
    origin: String,
    limits: RuntimeLimits,
    tls_roots: Option<RootCertStore>,
) -> (Host<()>, PluginHandle) {
    let builder = HostBuilder::new(()).unwrap();
    let mut builder = match tls_roots {
        Some(roots) => builder.tls_roots(roots),
        None => builder,
    };
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
            limits,
            InvocationCtx::bounded(common::INVOCATION_FUEL, common::INVOCATION_DEADLINE),
        )
        .await
        .unwrap();
    (builder.finish(), plugin)
}

#[tokio::test(flavor = "current_thread")]
async fn https_with_default_roots_rejects_private_ca() {
    let server = serve_tls_once().await;
    let (host, plugin) = admitted_client(server.origin.clone()).await;
    let guest = host.guest(&plugin).unwrap();

    let error = tokio::time::timeout(
        Duration::from_secs(5),
        guest.get(
            InvocationCtx::bounded(common::INVOCATION_FUEL, common::INVOCATION_DEADLINE),
            &format!("{}/private", server.origin),
        ),
    )
    .await
    .expect("HTTPS request must not hang")
    .expect("TLS rejection must be a guest-visible wasi-http error")
    .expect_err("the private CA must not be trusted by default");

    assert!(error.contains("HTTP request failed"), "{error}");
    assert!(error.contains("TlsCertificateError"), "{error}");
    assert!(server.task.await.unwrap().is_err());
    host.shutdown().await;
}

#[tokio::test(flavor = "current_thread")]
async fn https_with_supplied_tls_root_accepts_private_ca() {
    let server = serve_tls_once().await;
    let (host, plugin) = admitted_client_with_options(
        server.origin.clone(),
        RuntimeLimits::default(),
        Some(server.roots),
    )
    .await;
    let guest = host.guest(&plugin).unwrap();

    let response = tokio::time::timeout(
        Duration::from_secs(5),
        guest.get(
            InvocationCtx::bounded(common::INVOCATION_FUEL, common::INVOCATION_DEADLINE),
            &format!("{}/private", server.origin),
        ),
    )
    .await
    .expect("HTTPS request must not hang")
    .unwrap()
    .unwrap();

    assert_eq!(response.status, 201);
    assert_eq!(response.body, RESPONSE_BODY);
    server.task.await.unwrap().unwrap();
    host.shutdown().await;
}

#[tokio::test(flavor = "current_thread")]
async fn wasi_stream_io_does_not_consume_the_guarded_call_limit() {
    let (allowed_origin, server) = serve_once();
    let (host, plugin) = admitted_client_with_limits(
        allowed_origin.clone(),
        RuntimeLimits {
            max_host_import_calls: 1,
            ..RuntimeLimits::default()
        },
    )
    .await;
    let guest = host.guest(&plugin).unwrap();

    let response = guest
        .get(
            InvocationCtx::bounded(common::INVOCATION_FUEL, common::INVOCATION_DEADLINE),
            &format!("{allowed_origin}/stream-limit"),
        )
        .await
        .unwrap()
        .unwrap();

    assert_eq!(response.status, 201);
    assert_eq!(response.body, RESPONSE_BODY);
    server.join().unwrap();
    host.shutdown().await;
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
