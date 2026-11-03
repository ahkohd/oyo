use anyhow::{anyhow, Result};
use serde_json::Value;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, OnceLock,
};

type TreePage = Arc<OnceLock<std::result::Result<Arc<Value>, String>>>;

#[derive(Default)]
struct TreeCache {
    pages: HashMap<String, TreePage>,
    bytes: usize,
}

#[derive(Default)]
struct Cancellation {
    flag: AtomicBool,
    children: Mutex<HashSet<u32>>,
}

impl Cancellation {
    fn cancel(&self) {
        let children = self.children.lock().unwrap_or_else(|e| e.into_inner());
        self.flag.store(true, Ordering::Release);
        for pid in children.iter() {
            stop_child(*pid);
        }
    }
}

fn stop_child(pid: u32) {
    #[cfg(unix)]
    unsafe {
        libc::kill(-(pid as i32), libc::SIGKILL);
    }
}

#[derive(Clone, Default)]
pub(crate) struct ReadContext {
    cancelled: Arc<Cancellation>,
    trees: Arc<Mutex<TreeCache>>,
    parent: Option<Arc<Cancellation>>,
    foreground: bool,
}

thread_local! {
    static CONTEXT: RefCell<Option<ReadContext>> = const { RefCell::new(None) };
}

impl ReadContext {
    pub(crate) fn child(&self, foreground: bool) -> Self {
        Self {
            cancelled: Arc::default(),
            trees: self.trees.clone(),
            parent: Some(self.cancelled.clone()),
            foreground,
        }
    }

    pub(crate) fn cancel(&self) {
        self.cancelled.cancel();
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancelled.flag.load(Ordering::Acquire)
            || self
                .parent
                .as_ref()
                .is_some_and(|p| p.flag.load(Ordering::Acquire))
    }

    pub(crate) fn run<T>(&self, work: impl FnOnce() -> T) -> T {
        struct Restore(Option<ReadContext>);
        impl Drop for Restore {
            fn drop(&mut self) {
                CONTEXT.with(|slot| slot.replace(self.0.take()));
            }
        }
        let _restore = Restore(CONTEXT.with(|slot| slot.replace(Some(self.clone()))));
        work()
    }
}

pub(crate) fn current() -> ReadContext {
    CONTEXT
        .with(|slot| slot.borrow().clone())
        .unwrap_or_default()
}

pub(crate) fn cancelled() -> bool {
    CONTEXT.with(|slot| {
        slot.borrow()
            .as_ref()
            .is_some_and(ReadContext::is_cancelled)
    })
}

pub(super) fn tree_page(key: String, fetch: impl FnOnce() -> Result<Value>) -> Result<Arc<Value>> {
    let context = CONTEXT.with(|slot| slot.borrow().clone());
    let Some(context) = context else {
        return fetch().map(Arc::new);
    };
    if context.is_cancelled() {
        return Err(anyhow!("Remote loading cancelled"));
    }
    let page = {
        let mut cache = context.trees.lock().unwrap_or_else(|e| e.into_inner());
        cache.pages.entry(key.clone()).or_default().clone()
    };
    let mut fetched_bytes = None;
    let shared_context = ReadContext {
        cancelled: context
            .parent
            .clone()
            .unwrap_or_else(|| context.cancelled.clone()),
        trees: context.trees.clone(),
        parent: None,
        foreground: context.foreground,
    };
    let result = page
        .get_or_init(|| {
            shared_context
                .run(fetch)
                .map(|value| {
                    fetched_bytes = Some(value.to_string().len());
                    Arc::new(value)
                })
                .map_err(|e| e.to_string())
        })
        .clone();
    if fetched_bytes.is_some() || result.is_err() {
        let mut cache = context.trees.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(bytes) = fetched_bytes {
            cache.bytes = cache.bytes.saturating_add(bytes);
            if cache.bytes > 64 * 1024 * 1024 {
                cache.pages.retain(|_, page| page.get().is_none());
                cache.bytes = 0;
            }
        } else {
            if cache
                .pages
                .get(&key)
                .is_some_and(|entry| Arc::ptr_eq(entry, &page))
            {
                cache.pages.remove(&key);
            }
        }
    }
    result.map_err(|error| anyhow!(error))
}

static TRANSPORTS: Mutex<(usize, usize)> = Mutex::new((0, 0));

pub(crate) struct TransportPermit {
    foreground: bool,
    admitted: bool,
}

impl TransportPermit {
    pub(crate) fn acquire() -> Result<Self> {
        let context = CONTEXT.with(|slot| slot.borrow().clone());
        let Some(context) = context else {
            return Ok(Self {
                foreground: false,
                admitted: false,
            });
        };
        let foreground = context.foreground;
        loop {
            if cancelled() {
                return Err(anyhow!("Remote loading cancelled"));
            }
            {
                let mut active = TRANSPORTS.lock().unwrap_or_else(|e| e.into_inner());
                if active.0 < 6 && (foreground || active.1 < 4) {
                    active.0 += 1;
                    active.1 += usize::from(!foreground);
                    return Ok(Self {
                        foreground,
                        admitted: true,
                    });
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
}

impl Drop for TransportPermit {
    fn drop(&mut self) {
        if !self.admitted {
            return;
        }
        let mut active = TRANSPORTS.lock().unwrap_or_else(|e| e.into_inner());
        active.0 -= 1;
        active.1 -= usize::from(!self.foreground);
    }
}

pub(crate) struct RegisteredChild {
    context: ReadContext,
    child: std::process::Child,
    registered: bool,
}

impl RegisteredChild {
    pub(crate) fn spawn(command: &mut std::process::Command) -> Result<Self> {
        Self::spawn_with(|| command.spawn())
    }

    fn spawn_with(spawn: impl FnOnce() -> std::io::Result<std::process::Child>) -> Result<Self> {
        let context = current();
        let child = {
            let mut parent = context
                .parent
                .as_ref()
                .map(|p| p.children.lock().unwrap_or_else(|e| e.into_inner()));
            let mut children = context
                .cancelled
                .children
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if context.is_cancelled() {
                return Err(anyhow!("Remote loading cancelled"));
            }
            let child = spawn()?;
            children.insert(child.id());
            if let Some(parent) = &mut parent {
                parent.insert(child.id());
            }
            child
        };
        Ok(Self {
            context,
            child,
            registered: true,
        })
    }

    pub(crate) fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
        let mut parent = self
            .context
            .parent
            .as_ref()
            .map(|p| p.children.lock().unwrap_or_else(|e| e.into_inner()));
        let mut children = self
            .context
            .cancelled
            .children
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let status = self.child.try_wait()?;
        if status.is_some() {
            children.remove(&self.child.id());
            if let Some(parent) = &mut parent {
                parent.remove(&self.child.id());
            }
            self.registered = false;
        }
        Ok(status)
    }

    pub(crate) fn stop(&mut self) {
        let mut parent = self
            .context
            .parent
            .as_ref()
            .map(|p| p.children.lock().unwrap_or_else(|e| e.into_inner()));
        let mut children = self
            .context
            .cancelled
            .children
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if self.registered {
            stop_child(self.child.id());
            let _ = self.child.kill();
            let _ = self.child.wait();
            children.remove(&self.child.id());
            if let Some(parent) = &mut parent {
                parent.remove(&self.child.id());
            }
            self.registered = false;
        }
    }
}

impl std::ops::Deref for RegisteredChild {
    type Target = std::process::Child;
    fn deref(&self) -> &Self::Target {
        &self.child
    }
}

impl std::ops::DerefMut for RegisteredChild {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.child
    }
}

impl Drop for RegisteredChild {
    fn drop(&mut self) {
        self.stop();
    }
}

pub(crate) struct SnapshotWorker {
    rx: std::sync::mpsc::Receiver<std::result::Result<crate::BuiltDiff, String>>,
    context: ReadContext,
}

impl SnapshotWorker {
    pub(crate) fn start(url: String) -> Self {
        let context = ReadContext::default();
        let worker = context.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            worker.run(|| {
                let _ = tx.send(super::build(&url).map_err(|e| e.to_string()));
            })
        });
        Self { rx, context }
    }

    pub(crate) fn try_recv(
        &self,
    ) -> std::result::Result<
        std::result::Result<crate::BuiltDiff, String>,
        std::sync::mpsc::TryRecvError,
    > {
        self.rx.try_recv()
    }
}

impl Drop for SnapshotWorker {
    fn drop(&mut self) {
        self.context.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn concurrent_tree_readers_share_one_fetch() {
        let context = ReadContext::default();
        let fetches = AtomicUsize::new(0);
        std::thread::scope(|scope| {
            for _ in 0..6 {
                let context = &context;
                let fetches = &fetches;
                scope.spawn(move || {
                    context.run(|| {
                        let page = tree_page("immutable-root".into(), || {
                            fetches.fetch_add(1, Ordering::SeqCst);
                            std::thread::sleep(std::time::Duration::from_millis(10));
                            Ok(serde_json::json!({"tree": []}))
                        })
                        .unwrap();
                        assert_eq!(page["tree"], serde_json::json!([]));
                    })
                });
            }
        });
        assert_eq!(fetches.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn unscoped_sync_transport_does_not_queue_behind_loading_reads() {
        let context = ReadContext::default();
        let permits = context.run(|| {
            (0..4)
                .map(|_| TransportPermit::acquire().unwrap())
                .collect::<Vec<_>>()
        });
        let (tx, rx) = std::sync::mpsc::channel();
        let sync = std::thread::spawn(move || {
            let permit = TransportPermit::acquire().unwrap();
            tx.send(()).unwrap();
            drop(permit);
        });
        let admitted = rx.recv_timeout(std::time::Duration::from_millis(200));
        drop(permits);
        sync.join().unwrap();
        assert!(admitted.is_ok());
    }

    #[test]
    #[cfg(unix)]
    fn cancellation_waits_for_registered_spawn_and_stops_the_child() {
        use std::os::unix::process::CommandExt;
        use std::sync::mpsc;
        use std::time::Duration;
        let context = ReadContext::default();
        let worker = context.clone();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (child_tx, child_rx) = mpsc::channel();
        let spawn = std::thread::spawn(move || {
            worker.run(|| {
                let child = RegisteredChild::spawn_with(|| {
                    entered_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    std::process::Command::new("sh")
                        .args(["-c", "sleep 30"])
                        .process_group(0)
                        .spawn()
                })
                .unwrap();
                child_tx.send(child).unwrap();
            })
        });
        entered_rx.recv().unwrap();
        assert!(context.cancelled.children.try_lock().is_err());
        let (cancel_tx, cancel_rx) = mpsc::channel();
        let cancel = std::thread::spawn(move || {
            context.cancel();
            cancel_tx.send(()).unwrap();
        });
        assert!(cancel_rx.recv_timeout(Duration::from_millis(20)).is_err());
        release_tx.send(()).unwrap();
        cancel_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let mut child = child_rx.recv().unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        while child.try_wait().unwrap().is_none() {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        spawn.join().unwrap();
        cancel.join().unwrap();
    }

    #[test]
    fn failed_page_can_be_retried_and_child_cancellation_is_local() {
        let context = ReadContext::default();
        context.run(|| {
            assert!(tree_page("root".into(), || Err(anyhow!("temporary failure"))).is_err());
            assert!(tree_page("root".into(), || Ok(serde_json::json!({}))).is_ok());
        });
        let first = context.child(false);
        let second = context.child(false);
        first.cancel();
        assert!(first.is_cancelled());
        assert!(!second.is_cancelled());
        context.cancel();
        assert!(second.is_cancelled());
    }
}
