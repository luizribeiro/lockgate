//! Host-wide HTTP/1 connection pooling for wasi-http requests.

use std::{
    collections::HashMap,
    future::Future,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    task::{Poll, ready},
    time::Duration,
};

use ::http::{Request, Response, Uri, uri::Scheme};
use http_body::Body;
use http_body_util::BodyExt as _;
use hyper::client::conn::http1::SendRequest;
use rustls::{ClientConfig, RootCertStore, pki_types::ServerName};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::TcpStream,
    sync::{Mutex as AsyncMutex, OwnedMutexGuard},
    task::JoinHandle,
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

/// One transport pool shared by every Store belonging to a Host.
///
/// Keys contain only the URI scheme and authority. Hyper's sender retains no
/// request headers, credentials, or bodies after a request completes.
pub(crate) struct HttpPool {
    state: Mutex<PoolState>,
}

struct PoolState {
    roots: Option<Arc<RootCertStore>>,
    connections: HashMap<Uri, Vec<Arc<PooledConnection>>>,
    shutdown: bool,
}

struct PooledConnection {
    sender: Arc<AsyncMutex<SendRequest<WasiBody>>>,
    driver: Mutex<Option<JoinHandle<()>>>,
    busy: AtomicBool,
    closed: Arc<AtomicBool>,
}

struct ConnectionLease {
    connection: Option<Arc<PooledConnection>>,
}

impl HttpPool {
    pub(crate) fn new() -> Self {
        Self {
            state: Mutex::new(PoolState {
                roots: None,
                connections: HashMap::new(),
                shutdown: false,
            }),
        }
    }

    pub(crate) fn set_tls_roots(&self, roots: Arc<RootCertStore>) {
        let connections = {
            let mut state = self.state.lock().expect("HTTP pool lock poisoned");
            state.roots = Some(roots);
            std::mem::take(&mut state.connections)
        };
        for connection in connections.into_values().flatten() {
            connection.abort();
        }
    }

    pub(crate) fn begin_shutdown(&self) {
        let connections = {
            let mut state = self.state.lock().expect("HTTP pool lock poisoned");
            state.shutdown = true;
            state
                .connections
                .values()
                .flatten()
                .cloned()
                .collect::<Vec<_>>()
        };
        for connection in connections {
            connection.abort();
        }
    }

    pub(crate) async fn shutdown(&self) {
        self.begin_shutdown();
        let connections = {
            let mut state = self.state.lock().expect("HTTP pool lock poisoned");
            std::mem::take(&mut state.connections)
                .into_values()
                .flatten()
                .collect::<Vec<_>>()
        };
        for connection in connections {
            if let Some(driver) = connection.take_driver() {
                driver.abort();
                let _ = driver.await;
            }
        }
    }

    pub(crate) async fn send_request(
        &self,
        mut request: Request<WasiBody>,
        options: Option<RequestOptions>,
    ) -> Result<
        (
            Response<WasiBody>,
            Box<dyn Future<Output = Result<(), Error>> + Send>,
        ),
        Error,
    > {
        let key = pool_key(request.uri())?;
        let connect_timeout = options
            .and_then(|options| options.connect_timeout)
            .unwrap_or(Duration::from_secs(600));
        let first_byte_timeout = options
            .and_then(|options| options.first_byte_timeout)
            .unwrap_or(Duration::from_secs(600));
        let between_bytes_timeout = options
            .and_then(|options| options.between_bytes_timeout)
            .unwrap_or(Duration::from_secs(600));

        let (lease, mut sender) = self.checkout(&key, connect_timeout).await?;
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

        let response = tokio::time::timeout(first_byte_timeout, sender.send_request(request))
            .await
            .map_err(|_| Error::ConnectionReadTimeout)?
            .map_err(Error::from)?;
        drop(sender);

        let connection = lease.into_connection();
        let mut timeout = tokio::time::interval(between_bytes_timeout);
        timeout.reset();
        let response = response.map(|incoming| {
            IncomingResponseBody::new(incoming, timeout, connection).boxed_unsync()
        });
        Ok((response, Box::new(std::future::ready(Ok(())))))
    }

    async fn checkout(
        &self,
        key: &Uri,
        connect_timeout: Duration,
    ) -> Result<(ConnectionLease, OwnedMutexGuard<SendRequest<WasiBody>>), Error> {
        let candidates = {
            let mut state = self.state.lock().expect("HTTP pool lock poisoned");
            if state.shutdown {
                return Err(Error::InternalError(Some(
                    "HTTP connection pool is shut down".to_owned(),
                )));
            }
            let connections = state.connections.entry(key.clone()).or_default();
            connections.retain(|connection| !connection.is_closed());
            connections.clone()
        };

        for connection in candidates {
            let Some(lease) = ConnectionLease::acquire(connection) else {
                continue;
            };
            let mut sender = lease.sender().lock_owned().await;
            match sender.ready().await {
                Ok(()) => return Ok((lease, sender)),
                Err(_) => drop(lease),
            }
        }

        self.connect(key, connect_timeout).await
    }

    async fn connect(
        &self,
        key: &Uri,
        connect_timeout: Duration,
    ) -> Result<(ConnectionLease, OwnedMutexGuard<SendRequest<WasiBody>>), Error> {
        let roots = {
            let state = self.state.lock().expect("HTTP pool lock poisoned");
            if state.shutdown {
                return Err(Error::InternalError(Some(
                    "HTTP connection pool is shut down".to_owned(),
                )));
            }
            state.roots.clone()
        };
        let scheme = key.scheme().expect("pool keys always contain a scheme");
        let authority = key
            .authority()
            .expect("pool keys always contain an authority");
        let address = if authority.port().is_some() {
            authority.to_string()
        } else {
            let port = if scheme == &Scheme::HTTPS { 443 } else { 80 };
            format!("{authority}:{port}")
        };

        let stream = match tokio::time::timeout(connect_timeout, TcpStream::connect(&address)).await
        {
            Ok(stream) => stream.map_err(Error::Connect)?,
            Err(_) => return Err(Error::ConnectionTimeout),
        };
        let stream = if scheme == &Scheme::HTTPS {
            let roots = roots.unwrap_or_else(|| {
                Arc::new(RootCertStore {
                    roots: webpki_roots::TLS_SERVER_ROOTS.into(),
                })
            });
            // Keep Wasmtime's provider selection: the embedder must install one.
            let config = ClientConfig::builder()
                .with_root_certificates((*roots).clone())
                .with_no_client_auth();
            let connector = TlsConnector::from(Arc::new(config));
            let domain = tls_server_name(&address)?;
            connector
                .connect(domain, stream)
                .await
                .map_err(Error::Tls)?
                .boxed()
        } else {
            stream.boxed()
        };
        let (sender, connection) = tokio::time::timeout(
            connect_timeout,
            hyper::client::conn::http1::Builder::new().handshake(TokioIo::new(stream)),
        )
        .await
        .map_err(|_| Error::ConnectionTimeout)??;

        let closed = Arc::new(AtomicBool::new(false));
        let driver_closed = Arc::clone(&closed);
        let driver = tokio::spawn(async move {
            let _ = connection.await;
            driver_closed.store(true, Ordering::Release);
        });
        let sender = Arc::new(AsyncMutex::new(sender));
        let connection = Arc::new(PooledConnection {
            sender: Arc::clone(&sender),
            driver: Mutex::new(Some(driver)),
            busy: AtomicBool::new(true),
            closed,
        });
        let mut sender = sender.lock_owned().await;
        tokio::time::timeout(connect_timeout, sender.ready())
            .await
            .map_err(|_| Error::ConnectionTimeout)??;

        {
            let mut state = self.state.lock().expect("HTTP pool lock poisoned");
            if state.shutdown {
                return Err(Error::InternalError(Some(
                    "HTTP connection pool is shut down".to_owned(),
                )));
            }
            state
                .connections
                .entry(key.clone())
                .or_default()
                .push(Arc::clone(&connection));
        }
        Ok((ConnectionLease::new(connection), sender))
    }
}

impl Drop for HttpPool {
    fn drop(&mut self) {
        self.begin_shutdown();
    }
}

impl PooledConnection {
    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    fn abort(&self) {
        self.closed.store(true, Ordering::Release);
        if let Some(driver) = self
            .driver
            .lock()
            .expect("HTTP driver lock poisoned")
            .as_ref()
        {
            driver.abort();
        }
    }

    fn take_driver(&self) -> Option<JoinHandle<()>> {
        self.driver
            .lock()
            .expect("HTTP driver lock poisoned")
            .take()
    }

    fn release(&self, reusable: bool) {
        if !reusable {
            self.abort();
        }
        self.busy.store(false, Ordering::Release);
    }
}

impl Drop for PooledConnection {
    fn drop(&mut self) {
        self.abort();
    }
}

impl ConnectionLease {
    fn new(connection: Arc<PooledConnection>) -> Self {
        Self {
            connection: Some(connection),
        }
    }

    fn acquire(connection: Arc<PooledConnection>) -> Option<Self> {
        connection
            .busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| Self::new(connection))
    }

    fn sender(&self) -> Arc<AsyncMutex<SendRequest<WasiBody>>> {
        Arc::clone(
            &self
                .connection
                .as_ref()
                .expect("active lease always contains a connection")
                .sender,
        )
    }

    fn into_connection(mut self) -> Arc<PooledConnection> {
        self.connection
            .take()
            .expect("active lease always contains a connection")
    }
}

impl Drop for ConnectionLease {
    fn drop(&mut self) {
        if let Some(connection) = self.connection.take() {
            connection.release(false);
        }
    }
}

struct IncomingResponseBody {
    incoming: hyper::body::Incoming,
    timeout: tokio::time::Interval,
    connection: Option<Arc<PooledConnection>>,
}

impl IncomingResponseBody {
    fn new(
        incoming: hyper::body::Incoming,
        timeout: tokio::time::Interval,
        connection: Arc<PooledConnection>,
    ) -> Self {
        let mut body = Self {
            incoming,
            timeout,
            connection: Some(connection),
        };
        if body.incoming.is_end_stream() {
            body.release(true);
        }
        body
    }

    fn release(&mut self, reusable: bool) {
        if let Some(connection) = self.connection.take() {
            connection.release(reusable);
        }
    }
}

impl Drop for IncomingResponseBody {
    fn drop(&mut self) {
        self.release(false);
    }
}

impl Body for IncomingResponseBody {
    type Data = bytes::Bytes;
    type Error = Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
        match Pin::new(&mut self.as_mut().incoming).poll_frame(cx) {
            Poll::Ready(None) => {
                self.release(true);
                Poll::Ready(None)
            }
            Poll::Ready(Some(Err(error))) => {
                self.release(false);
                let error = if error.is_timeout() {
                    Error::HttpResponseTimeout
                } else {
                    Error::from(error)
                };
                Poll::Ready(Some(Err(error)))
            }
            Poll::Ready(Some(Ok(frame))) => {
                self.timeout.reset();
                if self.incoming.is_end_stream() {
                    self.release(true);
                }
                Poll::Ready(Some(Ok(frame)))
            }
            Poll::Pending => {
                ready!(self.timeout.poll_tick(cx));
                self.release(false);
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

fn pool_key(uri: &Uri) -> Result<Uri, Error> {
    let scheme = uri.scheme().ok_or(Error::HttpRequestUriInvalid)?;
    let authority = uri.authority().ok_or(Error::HttpRequestUriInvalid)?;
    Uri::builder()
        .scheme(scheme.clone())
        .authority(authority.clone())
        .path_and_query("/")
        .build()
        .map_err(|_| Error::HttpRequestUriInvalid)
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
