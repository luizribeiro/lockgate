//! Mediated outbound HTTP policy.

use std::{
    any::Any,
    future::Future,
    panic::{AssertUnwindSafe, catch_unwind},
    str::FromStr,
    sync::Arc,
    time::Duration,
};

use ::http::{Request, Response, Uri};
use lockgate_policy::{HttpOrigin, net};
use wasmtime_wasi_http::{
    Error as WasiHttpError, RequestOptions, WasiBody, WasiHttpHooks, default_hooks,
};

use super::errors::{HostPanicState, catch_unwind_future};

#[cfg(not(test))]
use crate::PluginHandle;
#[cfg(test)]
use lockgate::PluginHandle;

pub(crate) struct HttpHooks {
    plugin: Option<Arc<dyn Any + Send + Sync>>,
    request_timeout_ceiling: Option<Duration>,
    host_panics: HostPanicState,
}

impl HttpHooks {
    #[allow(
        dead_code,
        reason = "unit policy tests construct hooks without an enclosing Store"
    )]
    pub(crate) fn new(
        plugin: Option<Arc<dyn Any + Send + Sync>>,
        request_timeout_ceiling: Option<Duration>,
    ) -> Self {
        Self::with_host_panics(plugin, request_timeout_ceiling, HostPanicState::default())
    }

    pub(crate) fn with_host_panics(
        plugin: Option<Arc<dyn Any + Send + Sync>>,
        request_timeout_ceiling: Option<Duration>,
        host_panics: HostPanicState,
    ) -> Self {
        Self {
            plugin,
            request_timeout_ceiling,
            host_panics,
        }
    }

    fn allows(&self, uri: &Uri) -> bool {
        let Some(origin) = request_origin(uri) else {
            return false;
        };
        self.plugin
            .as_deref()
            .and_then(|plugin| plugin.downcast_ref::<PluginHandle>())
            .is_some_and(|plugin| plugin.__scoped_access_allowed(net::EGRESS, &[origin]))
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
        let host_panics = self.host_panics.clone();
        let future = catch_unwind(AssertUnwindSafe(|| {
            if !self.allows(request.uri()) {
                return Box::new(async { Err(WasiHttpError::HttpRequestDenied) }) as Box<_>;
            }
            let options = clamp_request_options(options, self.request_timeout_ceiling);
            default_hooks().send_request(request, options, fut)
        }));
        let future = match future {
            Ok(future) => future,
            Err(payload) => {
                let error = http_panic_error(&host_panics, payload);
                return Box::new(async move { Err(error) });
            }
        };

        Box::new(async move {
            let (response, io) = catch_http_future(&host_panics, Box::into_pin(future)).await?;
            let io_host_panics = host_panics.clone();
            let io = Box::new(
                async move { catch_http_future(&io_host_panics, Box::into_pin(io)).await },
            ) as Box<dyn Future<Output = Result<(), WasiHttpError>> + Send>;
            Ok((response, io))
        })
    }
}

const HTTP_SEND_IMPORT: &str = "wasi:http/client@0.3.0#send";

pub(crate) async fn catch_http_future<T>(
    host_panics: &HostPanicState,
    future: impl Future<Output = Result<T, WasiHttpError>>,
) -> Result<T, WasiHttpError> {
    match catch_unwind_future(future).await {
        Ok(result) => result,
        Err(payload) => Err(http_panic_error(host_panics, payload)),
    }
}

fn http_panic_error(host_panics: &HostPanicState, payload: Box<dyn Any + Send>) -> WasiHttpError {
    let message = host_panics.record(HTTP_SEND_IMPORT, payload);
    WasiHttpError::InternalError(Some(format!("host import panicked: {message}")))
}

pub(crate) fn clamp_request_options(
    options: Option<RequestOptions>,
    ceiling: Option<Duration>,
) -> Option<RequestOptions> {
    let Some(ceiling) = ceiling else {
        return options;
    };
    let mut options = options.unwrap_or_default();
    options.connect_timeout = Some(options.connect_timeout.unwrap_or(ceiling).min(ceiling));
    options.first_byte_timeout = Some(options.first_byte_timeout.unwrap_or(ceiling).min(ceiling));
    options.between_bytes_timeout = Some(
        options
            .between_bytes_timeout
            .unwrap_or(ceiling)
            .min(ceiling),
    );
    Some(options)
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
