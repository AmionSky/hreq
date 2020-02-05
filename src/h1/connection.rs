use super::task::{RecvBody, RecvRes, SendBody, SendReq, Seq, Task};
use super::Error;
use super::Inner;
use super::State;
use super::{AsyncRead, AsyncWrite};
use futures_util::ready;
use std::future::Future;
use std::io;
use std::mem;
use std::pin::Pin;
use std::sync::{Arc, Mutex, Weak};
use std::task::{Context, Poll};

pub struct Connection<S> {
    io: S,
    inner: Weak<Mutex<Inner>>,
}

impl<S> Connection<S> {
    pub(crate) fn new(io: S, inner: &Arc<Mutex<Inner>>) -> Self {
        Connection {
            io,
            inner: Arc::downgrade(inner),
        }
    }
}

impl<S> Future for Connection<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    type Output = io::Result<()>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        let arc_opt = this.inner.upgrade();
        if arc_opt.is_none() {
            // all handles to operate on this connection are gone.
            // TODO preserve connection errors that are gone with Inner being dropped.
            return Ok(()).into();
        }
        let arc_mutex = arc_opt.unwrap();

        let mut inner = arc_mutex.lock().unwrap();

        inner.conn_waker = Some(cx.waker().clone());

        loop {
            let cur_seq = Seq(inner.cur_seq);
            let mut state = inner.state; // copy to appease borrow checker

            if state == State::Closed {
                if let Some(e) = inner.error.as_mut() {
                    let repl = io::Error::new(e.kind(), Error::Message(e.to_string()));
                    let orig = mem::replace(e, repl);
                    return Err(orig).into();
                } else {
                    return Ok(()).into();
                }
            }

            // prune before getting any task for the state, to avoid getting stale.
            inner.tasks.prune_completed();

            if let Some(task) = inner.tasks.task_for_state(cur_seq, state) {
                match ready!(task.poll_connection(cx, &mut this.io, &mut state)) {
                    Ok(_) => {
                        if inner.state != State::Ready && state == State::Ready {
                            inner.cur_seq += 1;
                            trace!("New cur_seq: {}", inner.cur_seq);
                        }
                        if inner.state != state {
                            inner.state = state;
                            trace!("State transitioned to: {:?}", state);
                        }
                    }
                    Err(err) => {
                        trace!("Connection error: {:?}", err);
                        task.info_mut().complete = true;
                        inner.error = Some(err);
                        inner.state = State::Closed;
                    }
                };
            } else {
                break Poll::Pending;
            }
        }
    }
}

trait ConnectionPoll {
    fn poll_connection<S>(
        &mut self,
        cx: &mut Context,
        io: &mut S,
        state: &mut State,
    ) -> Poll<io::Result<()>>
    where
        S: AsyncRead + AsyncWrite + Unpin;
}

impl ConnectionPoll for SendReq {
    fn poll_connection<S>(
        &mut self,
        cx: &mut Context,
        io: &mut S,
        state: &mut State,
    ) -> Poll<io::Result<()>>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        loop {
            let amount = ready!(Pin::new(&mut *io).poll_write(cx, &self.req[..]))?;
            if amount < self.req.len() {
                self.req = self.req.split_off(amount);
                continue;
            }
            break;
        }
        if self.end {
            *state = State::Waiting;
        } else {
            *state = State::SendBody;
        }

        self.info.complete = true;

        Ok(()).into()
    }
}

impl ConnectionPoll for SendBody {
    fn poll_connection<S>(
        &mut self,
        cx: &mut Context,
        io: &mut S,
        state: &mut State,
    ) -> Poll<io::Result<()>>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        loop {
            let amount = ready!(Pin::new(&mut *io).poll_write(cx, &self.body[..]))?;
            if amount < self.body.len() {
                self.body = self.body.split_off(amount);
                continue;
            }
            break;
        }
        // entire current send_body was sent, waker is for a
        // someone potentially waiting to send more.
        if let Some(waker) = self.send_waker.take() {
            waker.wake();
        }

        if self.end {
            *state = State::Waiting;
        }

        self.info.complete = true;

        Ok(()).into()
    }
}

impl ConnectionPoll for RecvRes {
    fn poll_connection<S>(
        &mut self,
        cx: &mut Context,
        io: &mut S,
        state: &mut State,
    ) -> Poll<io::Result<()>>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        const END_OF_HEADER: &[u8] = &[b'\r', b'\n', b'\r', b'\n'];
        let mut end_index = 0;
        let mut buf_index = 0;
        let mut one = [0_u8; 1];
        loop {
            if buf_index == self.buf.len() {
                // read one more char
                let amount = ready!(Pin::new(&mut &mut *io).poll_read(cx, &mut one[..]))?;
                if amount == 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "EOF before complete http11 header",
                    ))
                    .into();
                }
                self.buf.push(one[0]);
            }

            if self.buf[buf_index] == END_OF_HEADER[end_index] {
                end_index += 1;
            } else if end_index > 0 {
                end_index = 0;
            }

            if end_index == END_OF_HEADER.len() {
                // we found the end of header sequence
                break;
            }
            buf_index += 1;
        }

        *state = State::RecvBody;

        // in theory we're now have a complete header ending \r\n\r\n
        self.waker.clone().wake();

        Ok(()).into()
    }
}

impl ConnectionPoll for RecvBody {
    fn poll_connection<S>(
        &mut self,
        cx: &mut Context,
        io: &mut S,
        state: &mut State,
    ) -> Poll<io::Result<()>>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let cur_len = self.buf.len();
        self.buf.resize(self.read_max, 0);
        if cur_len == self.read_max {
            self.waker.clone().wake();
            return Poll::Pending;
        }
        let read = Pin::new(&mut *io).poll_read(cx, &mut self.buf[cur_len..]);
        if let Poll::Pending = read {
            self.buf.resize(cur_len, 0);
        }
        let amount = ready!(read)?;
        self.buf.resize(cur_len + amount, 0);

        trace!(
            "RecvBody read_max: {} amount: {} buf size: {}",
            self.read_max,
            amount,
            self.buf.len(),
        );

        if amount == 0 {
            self.end = true;
            if self.reuse_conn {
                *state = State::Ready;
            } else {
                *state = State::Closed;
            }
        }

        self.waker.clone().wake();
        Ok(()).into()
    }
}

impl ConnectionPoll for Task {
    fn poll_connection<S>(
        &mut self,
        cx: &mut Context,
        io: &mut S,
        state: &mut State,
    ) -> Poll<io::Result<()>>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        match self {
            Task::SendReq(t) => t.poll_connection(cx, io, state),
            Task::SendBody(t) => t.poll_connection(cx, io, state),
            Task::RecvRes(t) => t.poll_connection(cx, io, state),
            Task::RecvBody(t) => t.poll_connection(cx, io, state),
        }
    }
}
