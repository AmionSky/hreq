use crate::react::with_reactor;
use crate::tcp::INIT_TOKEN;
use mio::Token;
use std::future::Future;
use std::pin::Pin;
use std::task::Context;
use std::task::Poll;
use std::time::Instant;

pub struct Timer {
    token: Token,
    due: Instant,
}

impl Timer {
    pub fn new(due: Instant) -> Self {
        Timer {
            token: INIT_TOKEN,
            due,
        }
    }
}

impl Future for Timer {
    type Output = ();
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let now = Instant::now();
        if now >= this.due {
            return Poll::Ready(());
        }
        with_reactor(|r| {
            let waker = cx.waker().clone();
            r.register_timer(this.token, this.due, waker);
        });
        Poll::Pending
    }
}
