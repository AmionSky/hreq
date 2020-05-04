use crate::react::with_reactor;
use futures_io::AsyncRead;
use futures_io::AsyncWrite;
use mio::net::TcpStream as Tcp;
use mio::Token;
use std::cell::RefCell;
use std::io::Error;
use std::io::ErrorKind;
use std::io::Read;
use std::io::Write;
use std::net::Shutdown;
use std::net::SocketAddr;
use std::net::ToSocketAddrs;
use std::pin::Pin;
use std::task::Context;
use std::task::Poll;
use thread_local::ThreadLocal;

struct TcpStreamInner {
    state: State,
    token: Token,
}

enum State {
    Init,
    Addr(SocketAddr),
    Connected(Tcp),
}

pub static INIT_TOKEN: Token = Token(usize::max_value());

impl TcpStreamInner {
    fn new() -> Self {
        TcpStreamInner {
            state: State::Init,
            token: INIT_TOKEN,
        }
    }

    fn connected(&mut self) -> Result<&mut Tcp, Error> {
        if let State::Addr(a) = self.state {
            let mut tcp = Tcp::connect(a)?;
            let token = with_reactor(|r| r.register_io(&mut tcp));
            self.token = token;
            self.state = State::Connected(tcp);
        }
        if let State::Connected(tcp) = &mut self.state {
            Ok(tcp)
        } else {
            unreachable!()
        }
    }

    fn io_to_poll<T>(
        &mut self,
        read: bool,
        cx: &mut Context<'_>,
        res: Result<T, Error>,
    ) -> Poll<Result<T, Error>> {
        match res {
            Err(e) if e.kind() == ErrorKind::WouldBlock || e.kind() == ErrorKind::NotConnected => {
                with_reactor(|r| {
                    let waker = cx.waker().clone();
                    r.register_waker(self.token, read, waker);
                });
                Poll::Pending
            }
            Err(e) => Poll::Ready(Err(e)),
            Ok(v) => Poll::Ready(Ok(v)),
        }
    }
}

impl AsyncRead for TcpStreamInner {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<Result<usize, Error>> {
        let this = self.get_mut();
        let res = this.connected().and_then(|tcp| tcp.read(buf));
        this.io_to_poll(true, cx, res)
    }
}

impl AsyncWrite for TcpStreamInner {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<futures_io::Result<usize>> {
        let this = self.get_mut();
        let res = this.connected().and_then(|tcp| tcp.write(buf));
        this.io_to_poll(false, cx, res)
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<futures_io::Result<()>> {
        let this = self.get_mut();
        let res = this.connected().and_then(|tcp| tcp.flush());
        this.io_to_poll(false, cx, res)
    }
    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<futures_io::Result<()>> {
        let this = self.get_mut();
        let res = this
            .connected()
            .and_then(|tcp| tcp.shutdown(Shutdown::Both));
        this.io_to_poll(false, cx, res)
    }
}

pub struct TcpStream {
    inner: ThreadLocal<RefCell<TcpStreamInner>>,
}

impl TcpStream {
    fn new() -> Self {
        TcpStream {
            inner: ThreadLocal::new(),
        }
    }

    fn with_inner<F, R>(&mut self, f: F) -> R
    where
        F: FnOnce(&mut TcpStreamInner) -> R,
    {
        let mut inner = self
            .inner
            .get_or(|| RefCell::new(TcpStreamInner::new()))
            .borrow_mut();
        f(&mut *inner)
    }

    pub fn connect<A: ToSocketAddrs>(addr: A) -> Result<TcpStream, Error> {
        let addr = addr.to_socket_addrs()?.next();
        if let Some(addr) = addr {
            let mut st = TcpStream::new();
            st.with_inner(|inner| inner.state = State::Addr(addr));
            Ok(st)
        } else {
            Err(Error::new(ErrorKind::NotFound, "Address not found"))
        }
    }
}

impl AsyncRead for TcpStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<Result<usize, Error>> {
        let this = self.get_mut();
        this.with_inner(|inner| Pin::new(inner).poll_read(cx, buf))
    }
}

impl AsyncWrite for TcpStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<futures_io::Result<usize>> {
        let this = self.get_mut();
        this.with_inner(|inner| Pin::new(inner).poll_write(cx, buf))
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<futures_io::Result<()>> {
        let this = self.get_mut();
        this.with_inner(|inner| Pin::new(inner).poll_flush(cx))
    }
    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<futures_io::Result<()>> {
        let this = self.get_mut();
        this.with_inner(|inner| Pin::new(inner).poll_close(cx))
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn tcp_stream_is_send() {
        is_send(TcpStream::new());
    }

    fn is_send<S: Send>(_: S) {}
}
