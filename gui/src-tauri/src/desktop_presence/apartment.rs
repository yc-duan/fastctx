//! The thread Windows' notification platform is asked on. Its calls can wait
//! in a way that lets window messages through. On the main thread those
//! messages re-enter Tauri's event loop while it holds the lock its plugin
//! commands run under, and the window never answers again. Here they wait on
//! a thread of their own, in the multithreaded apartment for the life of the
//! app, so the handlers a shown toast keeps stay reachable.

use std::sync::mpsc::{Sender, channel};

use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};

type Job = Box<dyn FnOnce() + Send>;

pub struct Apartment {
    jobs: Sender<Job>,
}

impl Apartment {
    pub fn start() -> Result<Self, String> {
        let (jobs, received) = channel::<Job>();
        let (entered, joined) = channel();
        std::thread::Builder::new()
            .name("world-notifications".into())
            .spawn(move || {
                let apartment = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.ok();
                let ready = apartment.is_ok();
                let _ = entered.send(apartment.map_err(|e| format!("entering Windows' multithreaded apartment: {e}")));
                if ready {
                    for job in received {
                        job();
                    }
                }
            })
            .map_err(|e| format!("starting the notification thread: {e}"))?;
        joined.recv().map_err(|_| "the notification thread stopped as it started")??;
        Ok(Self { jobs })
    }

    /// Runs `job` on the thread, one at a time, in the order asked.
    pub async fn run<T: Send + 'static>(
        &self,
        job: impl FnOnce() -> Result<T, String> + Send + 'static,
    ) -> Result<T, String> {
        let (answer, answered) = tokio::sync::oneshot::channel();
        self.jobs
            .send(Box::new(move || {
                let _ = answer.send(job());
            }))
            .map_err(|_| "the notification thread has stopped")?;
        answered.await.map_err(|_| "the notification thread has stopped")?
    }
}
