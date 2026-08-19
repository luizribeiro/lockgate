//! Mediated outbound HTTP policy.

use std::{any::Any, future::Future, str::FromStr, sync::Arc};

use ::http::{Request, Response, Uri};
use lockgate_policy::HttpOrigin;
use wasmtime_wasi_http::{
    Error as WasiHttpError, RequestOptions, WasiBody, WasiHttpHooks, default_hooks,
};

use crate::{PluginHandle, http, policy::scoped_access_allowed};

pub(super) struct HttpHooks {
    plugin: Option<Arc<dyn Any + Send + Sync>>,
}

impl HttpHooks {
    pub(super) fn new(plugin: Option<Arc<dyn Any + Send + Sync>>) -> Self {
        Self { plugin }
    }

    fn allows(&self, uri: &Uri) -> bool {
        let Some(plugin) = self
            .plugin
            .as_deref()
            .and_then(|plugin| plugin.downcast_ref::<PluginHandle>())
        else {
            return false;
        };
        let Some(origin) = request_origin(uri) else {
            return false;
        };
        scoped_access_allowed(
            plugin.capability_registry(),
            plugin.effective_grants(),
            http::EGRESS,
            &[origin],
        )
    }
}

impl WasiHttpHooks for HttpHooks {
    fn send_request(
        &mut self,
        request: Request<WasiBody>,
        options: Option<RequestOptions>,
        fut: Box<dyn Future<Output = Result<(), WasiHttpError>> + Send>,
    ) -> Box<
        dyn Future<
                Output = Result<
                    (
                        Response<WasiBody>,
                        Box<dyn Future<Output = Result<(), WasiHttpError>> + Send>,
                    ),
                    WasiHttpError,
                >,
            > + Send,
    > {
        if !self.allows(request.uri()) {
            return Box::new(async { Err(WasiHttpError::HttpRequestDenied) });
        }
        default_hooks().send_request(request, options, fut)
    }
}

fn request_origin(uri: &Uri) -> Option<HttpOrigin> {
    let scheme = uri.scheme_str()?;
    let authority = uri.authority()?;
    let port = authority.port_u16().unwrap_or(match scheme {
        "http" => 80,
        "https" => 443,
        _ => return None,
    });
    let host = authority.host();
    let host = if host.contains(':') {
        format!("[{host}]")
    } else {
        host.to_owned()
    };
    HttpOrigin::from_str(&format!("{scheme}://{host}:{port}")).ok()
}

#[cfg(test)]
mod tests {
    use std::{
        any::Any,
        io::{Read, Write},
        net::TcpListener,
        str::FromStr,
        sync::Arc,
        thread,
        time::Duration,
    };

    use ::http::{Request, Response, StatusCode};
    use bytes::Bytes;
    use http_body_util::{BodyExt, Empty};
    use lockgate_policy::{HttpOrigin, ScopeRepr};
    use lockgate_schema::GrantSet;
    use wasmtime_wasi_http::{Error as WasiHttpError, WasiBody, WasiHttpHooks};

    use super::HttpHooks;
    use crate::{
        PluginHandle, http,
        policy::{CapabilityRegistry, EffectiveGrants, ResolvedNeeds},
    };

    fn hooks_with_origins(origins: &[HttpOrigin]) -> HttpHooks {
        let mut registry = CapabilityRegistry::default();
        registry.register::<http::Contract>().unwrap();
        let mut required = GrantSet::new();
        if !origins.is_empty() {
            required
                .insert_scopes(
                    "http.egress".parse().unwrap(),
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
        HttpHooks::new(Some(plugin))
    }

    fn empty_body() -> WasiBody {
        Empty::<Bytes>::new()
            .map_err(|never| match never {})
            .boxed_unsync()
    }

    async fn send(hooks: &mut HttpHooks, uri: &str) -> Result<Response<WasiBody>, WasiHttpError> {
        let request = Request::get(uri).body(empty_body()).unwrap();
        Box::into_pin(hooks.send_request(request, None, Box::new(async { Ok(()) })))
            .await
            .map(|(response, _io)| response)
    }

    fn serve_once(response: &'static [u8]) -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut connection, _) = listener.accept().unwrap();
            connection
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = [0; 1024];
            let _ = connection.read(&mut request).unwrap();
            connection.write_all(response).unwrap();
        });
        (format!("http://{address}"), server)
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
}
