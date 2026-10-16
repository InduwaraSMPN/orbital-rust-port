//! Main-thread bridge — the WPF `Dispatcher.BeginInvoke` equivalent.
//!
//! GPUI owns its state behind a `RefCell` that only the platform run loop may touch,
//! so every effect that starts on the hook thread, a tokio task, or a tray callback
//! has to be marshalled back onto the main thread. A single queue plus one GPUI task
//! that drains it is that marshalling point: `post` never blocks and never runs the
//! job inline, which is what `Dispatcher.BeginInvoke` does with a `Normal` priority.

use std::sync::{Arc, OnceLock};

use gpui::{App, Task};

/// A closure that runs on the GPUI main thread with the app context.
pub type MainThreadJob = Box<dyn FnOnce(&mut App) + Send>;

/// Sends `MainThreadJob`s to the main thread, where a GPUI task drains them.
#[derive(Debug)]
pub struct MainThreadBridge {
    jobs: tokio::sync::mpsc::UnboundedSender<MainThreadJob>,
}

/// The process-wide bridge, installed once by [`MainThreadBridge::install`].
static BRIDGE: OnceLock<Arc<MainThreadBridge>> = OnceLock::new();

impl MainThreadBridge {
    /// Creates the bridge (if it is not installed yet) and starts its drain task on
    /// the GPUI main thread. Returns the shared handle either way.
    pub fn install(cx: &App) -> Arc<Self> {
        if let Some(existing) = BRIDGE.get() {
            return Arc::clone(existing);
        }

        let (jobs, mut receiver) = tokio::sync::mpsc::unbounded_channel::<MainThreadJob>();
        let bridge = Arc::new(Self { jobs });
        // A losing race can only happen if two threads install at once; the first
        // `set` wins and its handle is the one handed to everybody.
        let _ = BRIDGE.set(Arc::clone(&bridge));

        // GPUI cancels a task when its handle is dropped (`async_task`'s `Task::drop`
        // → `set_canceled`), so the drain loop must be detached or it dies the
        // moment this function returns and every `post` lands on a channel nobody
        // reads. This is the port's `Dispatcher` — it has to outlive `install`.
        let drain: Task<()> = cx.spawn(async move |cx| {
            while let Some(job) = receiver.recv().await {
                cx.update(|cx| job(cx));
            }
        });
        drain.detach();

        bridge
    }

    /// Queues a job for the main thread. Dropped silently once the drain task ends
    /// (the app is shutting down), because the source's `Dispatcher.BeginInvoke`
    /// after `Shutdown` is equally inert.
    pub fn post(&self, job: MainThreadJob) {
        let _ = self.jobs.send(job);
    }

    /// A bridge whose drain loop nobody runs. Only for callers that run before the
    /// GPUI app exists, so their effects are dropped instead of panicking.
    pub fn uninstalled() -> Arc<Self> {
        let (jobs, mut receiver) = tokio::sync::mpsc::unbounded_channel::<MainThreadJob>();
        // Drain eagerly: without a GPUI run loop there is nowhere to run the jobs.
        std::thread::spawn(move || while receiver.blocking_recv().is_some() {});
        Arc::new(Self { jobs })
    }

    /// The installed bridge, or `None` when the UI stack is not running (headless).
    pub fn global() -> Option<Arc<Self>> {
        BRIDGE.get().cloned()
    }

    /// Posts `job` when a bridge exists; otherwise reports that it was skipped.
    pub fn post_or_skip(&self, job: impl FnOnce(&mut App) + Send + 'static) {
        self.post(Box::new(job));
    }
}

/// Queues `job` for the main thread when the UI stack is running.
pub(crate) fn post(job: impl FnOnce(&mut App) + Send + 'static) {
    if let Some(bridge) = MainThreadBridge::global() {
        bridge.post(Box::new(job));
    } else {
        tracing::debug!("main-thread bridge is not installed; skipping a UI job");
    }
}
