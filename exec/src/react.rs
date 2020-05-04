use mio::event::Source;
use mio::Events;
use mio::Interest;
use mio::Poll as MioPoll;
use mio::Token;
use std::cell::RefCell;
use std::collections::HashMap;
use std::collections::LinkedList;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::Context;
use std::task::Poll;
use std::task::Waker;
use std::time::Duration;
use std::time::Instant;

thread_local! {
    static REACTOR: RefCell<Reactor> = RefCell::new(Reactor::new());
}

pub fn with_reactor<F, R>(f: F) -> R
where
    F: FnOnce(&mut Reactor) -> R,
{
    REACTOR.with(|r| {
        let mut brw = r.borrow_mut();
        f(&mut *brw)
    })
}

static TOKENS: AtomicUsize = AtomicUsize::new(0);

fn new_token() -> Token {
    Token(TOKENS.fetch_add(1, Ordering::Relaxed))
}

pub struct Reactor {
    poll: MioPoll,
    events: Events,
    timers: LinkedList<Token>,
    wakers: HashMap<Token, Wakers>,
}

#[derive(Default)]
struct Wakers {
    due: Option<Instant>,
    read: Option<Waker>,
    write: Option<Waker>,
}

impl Wakers {
    fn new() -> Self {
        Wakers {
            ..Default::default()
        }
    }
}

impl Reactor {
    fn new() -> Self {
        Reactor {
            poll: MioPoll::new().unwrap(),
            events: Events::with_capacity(100),
            timers: LinkedList::new(),
            wakers: HashMap::new(),
        }
    }

    pub fn register_io<S>(&mut self, source: &mut S) -> Token
    where
        S: Source,
    {
        let token = new_token();
        let interests = Interest::READABLE.add(Interest::WRITABLE);
        self.poll
            .registry()
            .register(source, token, interests)
            .unwrap();
        token
    }

    pub fn register_waker(&mut self, token: Token, read: bool, waker: Waker) {
        let wak = self.wakers.entry(token).or_insert_with(Wakers::new);
        if read {
            wak.read = Some(waker);
        } else {
            wak.write = Some(waker);
        }
    }

    pub fn register_timer(&mut self, token: Token, due: Instant, waker: Waker) {
        let wak = self.wakers.entry(token).or_insert_with(Wakers::new);
        wak.read = Some(waker);
        wak.due = Some(due);
        self.timers.push_back(token);
    }

    fn run(&mut self) {
        if self.run_timers() {
            return;
        }
        self.run_io();
    }

    fn run_timers(&mut self) -> bool {
        let todo = self.timers.split_off(0);

        let mut any_woke = false;
        let now = Instant::now();
        for t in todo {
            if let Some(waks) = self.wakers.get_mut(&t) {
                let due = waks.due.unwrap();
                if now >= due {
                    waks.read.take().unwrap().wake();
                    any_woke = true;
                    self.wakers.remove(&t);
                    continue;
                }
            }
            self.timers.push_back(t);
        }

        any_woke
    }

    fn run_io(&mut self) {
        self.poll
            .poll(&mut self.events, Some(Duration::from_secs(0)))
            .unwrap();

        for e in &self.events {
            let token = e.token();
            if let Some(waks) = self.wakers.get_mut(&token) {
                if e.is_readable() {
                    if let Some(wak) = waks.read.take() {
                        wak.wake();
                    }
                }
                if e.is_writable() {
                    if let Some(wak) = waks.write.take() {
                        wak.wake();
                    }
                }
                if waks.read.is_none() && waks.write.is_none() {
                    self.wakers.remove(&token);
                }
            }
        }
    }
}

pub struct ReactorFut(u8);

impl ReactorFut {
    pub fn new() -> Self {
        ReactorFut(200)
    }
}

impl Future for ReactorFut {
    type Output = ();
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        if this.0 > 0 {
            cx.waker().wake_by_ref();
            this.0 -= 1;
            Poll::Pending
        } else {
            with_reactor(|r| r.run());
            crate::spawn(ReactorFut::new());
            Poll::Ready(())
        }
    }
}
