use futures_executor::LocalPool;
use futures_executor::LocalSpawner;
use futures_task::Spawn;
use std::cell::RefCell;
use std::future::Future;
use std::io::Error;
use std::time::Duration;
use std::time::Instant;

mod react;
mod tcp;
mod timer;

use react::ReactorFut;
pub use tcp::TcpStream;
use timer::Timer;

thread_local! {
    static POOL: PoolSpawn = {
        let pool = LocalPool::new();
        let spawn = pool.spawner().clone();
        spawn.spawn_obj(box_fut(ReactorFut::new()).into()).ok();
        PoolSpawn(RefCell::new(pool), spawn)
    };
}

struct PoolSpawn(RefCell<LocalPool>, LocalSpawner);

pub fn connect_tcp(addr: &str) -> Result<TcpStream, Error> {
    TcpStream::connect(addr)
}

pub async fn timeout(duration: Duration) {
    let due = Instant::now() + duration;
    Timer::new(due).await
}

fn box_fut<T>(task: T) -> Box<dyn Future<Output = ()> + Send>
where
    T: Future + Send + 'static,
{
    Box::new(async move {
        task.await;
    }) as Box<dyn Future<Output = ()> + Send>
}

pub fn spawn<T>(task: T)
where
    T: Future + Send + 'static,
{
    POOL.with(|p| p.1.spawn_obj(box_fut(task).into()).ok());
}

pub fn block_on<F: Future>(task: F) -> F::Output {
    POOL.with(|p| p.0.borrow_mut().run_until(task))
}
