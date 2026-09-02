//! Wasmtime-compatible HTTP sender with caller-supplied TLS roots.

use std::{
    future::{Future, poll_fn},
    pin::{Pin, pin},
    sync::Arc,
    task::{Poll, ready},
    time::Duration,
};

use ::http::{Request, Response, Uri, uri::Scheme};
use http_body::Body;
use http_body_util::BodyExt as _;
use rustls::{ClientConfig, RootCertStore, pki_types::ServerName};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::TcpStream,
};
use tokio_rustls::TlsConnector;
use wasmtime_wasi_http::{Error, RequestOptions, WasiBody, io::TokioIo};

trait TokioStream: AsyncRead + AsyncWrite + Send + Sync + Unpin + 'static {
    fn boxed(self) -> Box<dyn TokioStream>
    where
        Self: Sized,
    {
        Box::new(self)
    }
}

impl<T> TokioStream for T where T: AsyncRead + AsyncWrite + Send + Sync + Unpin + 'static {}

/// Mirrors Wasmtime 48.0.1's default sender, changing only the TLS root store.
pub(crate) async fn send_request(
    mut request: Request<WasiBody>,
    options: Option<RequestOptions>,
    roots: Arc<RootCertStore>,
) -> Result<
    (
        Response<WasiBody>,
        Box<dyn Future<Output = Result<(), Error>> + Send>,
    ),
    Error,
> {
    let uri = request.uri();
    let authority = uri.authority().ok_or(Error::HttpRequestUriInvalid)?;
    let use_tls = uri.scheme() == Some(&Scheme::HTTPS);
    let authority = if authority.port().is_some() {
        authority.to_string()
    } else {
        let port = if use_tls { 443 } else { 80 };
        format!("{authority}:{port}")
    };

    let connect_timeout = options
        .and_then(|options| options.connect_timeout)
        .unwrap_or(Duration::from_secs(600));
    let first_byte_timeout = options
        .and_then(|options| options.first_byte_timeout)
        .unwrap_or(Duration::from_secs(600));
    let between_bytes_timeout = options
        .and_then(|options| options.between_bytes_timeout)
        .unwrap_or(Duration::from_secs(600));

    let stream = match tokio::time::timeout(connect_timeout, TcpStream::connect(&authority)).await {
        Ok(stream) => stream.map_err(Error::Connect)?,
        Err(_) => return Err(Error::ConnectionTimeout),
    };
    let stream = if use_tls {
        // Keep Wasmtime's provider selection: the embedder must install one.
        let config = ClientConfig::builder()
            .with_root_certificates((*roots).clone())
            .with_no_client_auth();
        let connector = TlsConnector::from(Arc::new(config));
        let domain = tls_server_name(&authority)?;
        connector
            .connect(domain, stream)
            .await
            .map_err(Error::Tls)?
            .boxed()
    } else {
        stream.boxed()
    };
    let (mut sender, connection) = tokio::time::timeout(
        connect_timeout,
        hyper::client::conn::http1::Builder::new().handshake(TokioIo::new(stream)),
    )
    .await
    .map_err(|_| Error::ConnectionTimeout)??;

    *request.uri_mut() = Uri::builder()
        .path_and_query(
            request
                .uri()
                .path_and_query()
                .map(|path| path.as_str())
                .unwrap_or("/"),
        )
        .build()
        .expect("request already contains a valid URI");

    let send = async move {
        let response = tokio::time::timeout(first_byte_timeout, sender.send_request(request))
            .await
            .map_err(|_| Error::ConnectionReadTimeout)?
            .map_err(Error::from)?;
        let mut timeout = tokio::time::interval(between_bytes_timeout);
        timeout.reset();
        Ok(response.map(|incoming| IncomingResponseBody { incoming, timeout }))
    };
    let mut send = pin!(send);
    let mut connection = Some(connection);
    let response = poll_fn(|cx| match send.as_mut().poll(cx) {
        Poll::Ready(Ok(response)) => Poll::Ready(Ok(response)),
        Poll::Ready(Err(error)) => Poll::Ready(Err(error)),
        Poll::Pending => {
            let Some(future) = connection.as_mut() else {
                return Poll::Pending;
            };
            let result = ready!(Pin::new(future).poll(cx));
            connection = None;
            match result {
                Ok(()) => send.as_mut().poll(cx),
                Err(error) => Poll::Ready(Err(Error::from(error))),
            }
        }
    })
    .await?;
    let response = response.map(|body| body.boxed_unsync());

    Ok((
        response,
        Box::new(async move {
            let Some(connection) = connection.take() else {
                return Ok(());
            };
            if let Err(error) = connection.await {
                if error.is_timeout() {
                    return Err(Error::HttpResponseTimeout);
                }
                return Err(error.into());
            }
            Ok(())
        }),
    ))
}

struct IncomingResponseBody {
    incoming: hyper::body::Incoming,
    timeout: tokio::time::Interval,
}

impl Body for IncomingResponseBody {
    type Data = bytes::Bytes;
    type Error = Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
        match Pin::new(&mut self.as_mut().incoming).poll_frame(cx) {
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Ready(Some(Err(error))) => {
                let error = if error.is_timeout() {
                    Error::HttpResponseTimeout
                } else {
                    Error::from(error)
                };
                Poll::Ready(Some(Err(error)))
            }
            Poll::Ready(Some(Ok(frame))) => {
                self.timeout.reset();
                Poll::Ready(Some(Ok(frame)))
            }
            Poll::Pending => {
                ready!(self.timeout.poll_tick(cx));
                Poll::Ready(Some(Err(Error::ConnectionReadTimeout)))
            }
        }
    }

    fn is_end_stream(&self) -> bool {
        self.incoming.is_end_stream()
    }

    fn size_hint(&self) -> http_body::SizeHint {
        self.incoming.size_hint()
    }
}

fn tls_server_name(
    authority: &str,
) -> Result<ServerName<'static>, rustls::pki_types::InvalidDnsNameError> {
    if let Ok(address) = authority.parse::<std::net::SocketAddr>() {
        return Ok(ServerName::from(address.ip()));
    }
    let host = authority
        .split_once(':')
        .map_or(authority, |(host, _port)| host);
    Ok(ServerName::try_from(host)?.to_owned())
}
