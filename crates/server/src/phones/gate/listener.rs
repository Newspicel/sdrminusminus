use std::{
    future::Future,
    io,
    net::{Ipv4Addr, SocketAddr, TcpListener},
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};

use axum_server::{Handle, accept::Accept, tls_rustls::RustlsConfig};
use sdrmm_wire::phone::PHONE_PORT_IN_USE;
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    sync::watch,
    task::JoinHandle,
};

use crate::{AppState, auth::ListenerRole};

const DRAIN: Duration = Duration::from_secs(2);
const STOP_WAIT: Duration = Duration::from_secs(3);

pub(super) struct RunningListener {
    pub(super) port: u16,
    pub(super) pin: String,
    handle: Handle<SocketAddr>,
    task: JoinHandle<io::Result<()>>,
    cut: watch::Sender<()>,
}

impl RunningListener {
    pub(super) fn start(
        state: &AppState,
        port: u16,
        tls: Arc<rustls::ServerConfig>,
        pin: String,
    ) -> Result<Self, String> {
        let listener = TcpListener::bind((Ipv4Addr::UNSPECIFIED, port)).map_err(bind_failure)?;
        listener
            .set_nonblocking(true)
            .map_err(|error| error.to_string())?;
        let (cut, uncut) = watch::channel(());
        let handle = Handle::new();
        let server = axum_server::from_tcp_rustls(listener, RustlsConfig::from_config(tls))
            .map_err(|error| error.to_string())?
            .map(|tls| Cutting { inner: tls, uncut })
            .handle(handle.clone());
        let app = crate::app(state, ListenerRole::Phones, true);
        let task = tokio::spawn(async move { server.serve(app.into_make_service()).await });
        tracing::info!(port, "phone listener on");
        Ok(Self {
            port,
            pin,
            handle,
            task,
            cut,
        })
    }

    pub(super) async fn stop(self) {
        let Self {
            port,
            handle,
            task,
            cut,
            ..
        } = self;
        handle.graceful_shutdown(Some(DRAIN));
        match tokio::time::timeout(STOP_WAIT, task).await {
            Ok(Ok(Ok(()))) => tracing::info!(port, "phone listener off"),
            Ok(Ok(Err(error))) => tracing::warn!(%error, port, "phone listener ended badly"),
            Ok(Err(error)) => tracing::warn!(%error, port, "phone listener task failed"),
            Err(_) => tracing::warn!(port, "phone listener did not stop in time"),
        }
        drop(cut);
    }

    pub(super) fn ended(&self) -> bool {
        self.task.is_finished()
    }

    pub(super) fn halt(self) {
        self.handle.shutdown();
        drop(self.cut);
    }
}

fn bind_failure(error: io::Error) -> String {
    if error.kind() == io::ErrorKind::AddrInUse {
        PHONE_PORT_IN_USE.to_owned()
    } else {
        error.to_string()
    }
}

#[derive(Clone)]
struct Cutting<A> {
    inner: A,
    uncut: watch::Receiver<()>,
}

type Accepted<T, S> = Pin<Box<dyn Future<Output = io::Result<(Cut<T>, S)>> + Send>>;

impl<I, S, A> Accept<I, S> for Cutting<A>
where
    A: Accept<I, S>,
    A::Future: Send + 'static,
    A::Stream: 'static,
    A::Service: 'static,
{
    type Stream = Cut<A::Stream>;
    type Service = A::Service;
    type Future = Accepted<A::Stream, A::Service>;

    fn accept(&self, stream: I, service: S) -> Self::Future {
        let accepted = self.inner.accept(stream, service);
        let uncut = self.uncut.clone();
        Box::pin(async move {
            let (stream, service) = accepted.await?;
            Ok((Cut::new(stream, uncut), service))
        })
    }
}

pub(super) struct Cut<T> {
    inner: T,
    closing: Pin<Box<dyn Future<Output = ()> + Send>>,
    closed: bool,
}

impl<T> Cut<T> {
    pub(super) fn new(inner: T, mut uncut: watch::Receiver<()>) -> Self {
        Self {
            inner,
            closing: Box::pin(async move { while uncut.changed().await.is_ok() {} }),
            closed: false,
        }
    }

    fn is_cut(&mut self, cx: &mut Context<'_>) -> bool {
        if !self.closed && self.closing.as_mut().poll(cx).is_ready() {
            self.closed = true;
        }
        self.closed
    }
}

fn aborted<T>() -> Poll<io::Result<T>> {
    Poll::Ready(Err(io::ErrorKind::ConnectionAborted.into()))
}

impl<T: AsyncRead + Unpin> AsyncRead for Cut<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if self.is_cut(cx) {
            return aborted();
        }
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<T: AsyncWrite + Unpin> AsyncWrite for Cut<T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        if self.is_cut(cx) {
            return aborted();
        }
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        if self.is_cut(cx) {
            return aborted();
        }
        Pin::new(&mut self.inner).poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}
