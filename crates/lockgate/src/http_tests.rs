use std::{
    any::Any,
    io::{ErrorKind, Read, Write},
    net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener},
    str::FromStr,
    sync::Arc,
    thread,
    time::Duration,
};

use ::http::{Request, Response, StatusCode};
use http_body_util::{BodyExt, Empty};
use lockgate_policy::{HttpOrigin, ScopeRepr};
use lockgate_schema::GrantSet;
use wasmtime_wasi_http::{Error as WasiHttpError, RequestOptions, WasiBody, WasiHttpHooks};

use crate::{
    PluginHandle,
    exec::{
        ExecEngine, StoreCtx, add_http_to_linker,
        wasi_http::{HttpHooks, clamp_request_options},
    },
    net,
    policy::{CapabilityRegistry, EffectiveGrants, ResolvedNeeds},
};

fn hooks_with_origins(origins: &[HttpOrigin]) -> HttpHooks {
    let mut registry = CapabilityRegistry::default();
    registry.register::<net::Contract>().unwrap();
    let mut required = GrantSet::new();
    if !origins.is_empty() {
        required
            .insert_scopes(
                "net.egress".parse().unwrap(),
                origins.iter().map(ScopeRepr::canonical),
            )
            .unwrap();
    }
    let plugin = PluginHandle::for_policy_test_with_registry(
        "http-test",
        EffectiveGrants::from_resolved(ResolvedNeeds {
            required,
            optional: GrantSet::new(),
        }),
        registry,
    );
    let plugin: Arc<dyn Any + Send + Sync> = Arc::new(plugin);
    HttpHooks::new(Some(plugin), None)
}

const TIMEOUT_CEILING: Duration = Duration::from_secs(5);

fn request_options(timeout: Option<Duration>) -> RequestOptions {
    RequestOptions {
        connect_timeout: timeout,
        first_byte_timeout: timeout,
        between_bytes_timeout: timeout,
    }
}

#[test]
fn ceiling_populates_all_unset_guest_timeouts() {
    assert_eq!(
        clamp_request_options(None, Some(TIMEOUT_CEILING)),
        Some(request_options(Some(TIMEOUT_CEILING)))
    );
}

#[test]
fn ceiling_clamps_longer_guest_timeouts() {
    assert_eq!(
        clamp_request_options(
            Some(request_options(Some(Duration::from_secs(10)))),
            Some(TIMEOUT_CEILING),
        ),
        Some(request_options(Some(TIMEOUT_CEILING)))
    );
}

#[test]
fn ceiling_preserves_shorter_guest_timeouts() {
    let guest_timeout = Duration::from_secs(2);
    assert_eq!(
        clamp_request_options(
            Some(request_options(Some(guest_timeout))),
            Some(TIMEOUT_CEILING),
        ),
        Some(request_options(Some(guest_timeout)))
    );
}

#[test]
fn no_ceiling_preserves_guest_options() {
    let options = RequestOptions {
        connect_timeout: Some(Duration::from_secs(1)),
        first_byte_timeout: None,
        between_bytes_timeout: Some(Duration::from_secs(3)),
    };

    assert_eq!(clamp_request_options(Some(options), None), Some(options));
    assert_eq!(clamp_request_options(None, None), None);
}

fn empty_body() -> WasiBody {
    Empty::<bytes::Bytes>::new()
        .map_err(|never| match never {})
        .boxed_unsync()
}

async fn send(hooks: &mut HttpHooks, uri: &str) -> Result<Response<WasiBody>, WasiHttpError> {
    let request = Request::get(uri).body(empty_body()).unwrap();
    Box::into_pin(hooks.send_request(request, None, Box::new(async { Ok(()) })))
        .await
        .map(|(response, _io)| response)
}

fn serve_once(response: &'static [u8]) -> (String, thread::JoinHandle<SocketAddr>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (mut connection, peer) = listener.accept().unwrap();
        connection
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut request = [0; 1024];
        let _ = connection.read(&mut request).unwrap();
        connection.write_all(response).unwrap();
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

#[test]
fn wasi_http_provider_is_linked_only_for_an_egress_grant() {
    let engine = ExecEngine::new().unwrap();
    let mut denied = wasmtime::component::Linker::<StoreCtx<()>>::new(engine.engine());
    add_http_to_linker(&mut denied, false).unwrap();
    assert!(
        denied
            .instance("wasi:http/client@0.3.0")
            .unwrap()
            .func_wrap_concurrent("send", |_, (): ()| Box::pin(async { Ok(()) }))
            .is_ok(),
        "a component without an egress grant must have no wasi:http provider"
    );

    let mut allowed = wasmtime::component::Linker::<StoreCtx<()>>::new(engine.engine());
    add_http_to_linker(&mut allowed, true).unwrap();
    assert!(
        allowed
            .instance("wasi:http/client@0.3.0")
            .unwrap()
            .func_wrap_concurrent("send", |_, (): ()| Box::pin(async { Ok(()) }))
            .is_err(),
        "an egress grant must make the wasi:http provider available"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn granted_origin_is_reachable_and_other_origin_is_refused() {
    let (allowed, server) =
        serve_once(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n");
    let mut hooks = hooks_with_origins(&[HttpOrigin::from_str(&allowed).unwrap()]);

    assert_eq!(
        send(&mut hooks, &format!("{allowed}/path"))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    server.join().unwrap();
    assert!(matches!(
        send(&mut hooks, "https://blocked.example/path").await,
        Err(WasiHttpError::HttpRequestDenied)
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn redirect_to_non_granted_origin_is_refused_per_hop() {
    let (allowed, server) = serve_once(
        b"HTTP/1.1 302 Found\r\nlocation: https://blocked.example/landing\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
    );
    let mut hooks = hooks_with_origins(&[HttpOrigin::from_str(&allowed).unwrap()]);

    let response = send(&mut hooks, &format!("{allowed}/redirect"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FOUND);
    let location = response.headers()["location"].to_str().unwrap();
    server.join().unwrap();
    assert!(matches!(
        send(&mut hooks, location).await,
        Err(WasiHttpError::HttpRequestDenied)
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn alternate_ipv4_spellings_connect_only_to_the_checked_origin() {
    for alternate in ["0x7f000001", "127.1"] {
        let (allowed, server) =
            serve_once(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n");
        let granted = HttpOrigin::from_str(&allowed).unwrap();
        let port = allowed.parse::<::http::Uri>().unwrap().port_u16().unwrap();
        let alternate_origin = HttpOrigin::from_str(&format!("http://{alternate}:{port}")).unwrap();
        assert_eq!(alternate_origin, granted);
        let mut hooks = hooks_with_origins(&[granted]);

        assert_eq!(
            send(
                &mut hooks,
                &format!("http://{alternate}:{port}/alternate-ipv4")
            )
            .await
            .unwrap()
            .status(),
            StatusCode::OK
        );
        assert_eq!(server.join().unwrap().ip(), IpAddr::V4(Ipv4Addr::LOCALHOST));
    }
}

#[tokio::test(flavor = "current_thread")]
async fn userinfo_cannot_redirect_connection_away_from_the_checked_origin() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let granted = HttpOrigin::from_str(&format!("http://{address}")).unwrap();
    let mut hooks = hooks_with_origins(&[granted]);

    let result = send(
        &mut hooks,
        &format!("http://blocked.example@{address}/userinfo"),
    )
    .await;

    assert!(matches!(result, Err(WasiHttpError::Connect(_))));
    assert_no_connection(listener);
}
