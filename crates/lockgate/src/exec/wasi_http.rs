//! Mediated outbound HTTP policy.

use std::{any::Any, future::Future, str::FromStr, sync::Arc};

use ::http::{Request, Response, Uri};
use lockgate_policy::{HttpOrigin, net};
use wasmtime_wasi_http::{
    Error as WasiHttpError, RequestOptions, WasiBody, WasiHttpHooks, default_hooks,
};

#[cfg(not(test))]
use crate::PluginHandle;
#[cfg(test)]
use lockgate::PluginHandle;

pub(crate) struct HttpHooks {
    plugin: Option<Arc<dyn Any + Send + Sync>>,
}

impl HttpHooks {
    pub(crate) fn new(plugin: Option<Arc<dyn Any + Send + Sync>>) -> Self {
        Self { plugin }
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
