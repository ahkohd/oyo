use super::loading::{CancellationHandle, ReadContext, ReadPriority};
use anyhow::{anyhow, Result};
use oyo_core::multi::ContentSource;
use oyo_core::patch::BlobId;
use serde_json::Value;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FailureKind {
    Cancelled,
    Blocked,
    Missing,
    TooLarge,
    Other,
}

#[derive(Clone, Debug)]
pub(crate) struct ReadFailure {
    pub kind: FailureKind,
    pub message: String,
}

impl std::fmt::Display for ReadFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for ReadFailure {}
impl ReadFailure {
    pub fn from_error(error: anyhow::Error) -> Self {
        for cause in error.chain() {
            if let Some(failure) = cause.downcast_ref::<Self>() {
                return failure.clone();
            }
            if let Some(error) = cause.downcast_ref::<std::io::Error>() {
                if let Some(failure) = error.get_ref().and_then(|e| e.downcast_ref::<Self>()) {
                    return failure.clone();
                }
            }
        }
        Self {
            kind: FailureKind::Other,
            message: error.to_string(),
        }
    }
    pub fn cancelled() -> Self {
        Self {
            kind: FailureKind::Cancelled,
            message: "Remote loading cancelled".into(),
        }
    }
}

#[derive(Default)]
struct DemandState {
    contexts: Vec<Weak<ReadContext>>,
    closing: bool,
}
#[derive(Default)]
pub(super) struct Demands {
    state: Mutex<DemandState>,
}
impl Demands {
    fn add(&self, context: &Arc<ReadContext>) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.closing {
            return false;
        }
        state.contexts.push(Arc::downgrade(context));
        true
    }
    pub fn priority(&self) -> Option<ReadPriority> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.closing {
            return None;
        }
        state.contexts.retain(|context| context.strong_count() > 0);
        let priority = state
            .contexts
            .iter()
            .filter_map(Weak::upgrade)
            .filter(|c| !c.is_cancelled())
            .map(|c| c.priority())
            .min();
        if priority.is_none() {
            state.closing = true;
        }
        priority
    }
}

type SharedResult<T> = std::result::Result<Arc<T>, ReadFailure>;
struct Flight<T> {
    result: Mutex<Option<SharedResult<T>>>,
    producing: AtomicBool,
    foreground: bool,
    bytes: AtomicUsize,
    changed: Condvar,
    demands: Arc<Demands>,
    cancel: CancellationHandle,
}
impl<T> Flight<T> {
    fn finish(&self, value: SharedResult<T>, bytes: usize, replace: bool) {
        let mut result = self.result.lock().unwrap_or_else(|e| e.into_inner());
        if replace || result.is_none() {
            self.bytes.store(bytes, Ordering::Release);
            *result = Some(value);
            self.changed.notify_all();
        }
    }
    fn wait(&self, context: &ReadContext) -> Result<Arc<T>> {
        let mut result = self.result.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if context.is_cancelled() {
                return Err(ReadFailure::cancelled().into());
            }
            if let Some(value) = result.as_ref() {
                return value.clone().map_err(Into::into);
            }
            result = self
                .changed
                .wait_timeout(result, Duration::from_millis(5))
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }
}

struct Entry<K, T> {
    key: K,
    flight: Arc<Flight<T>>,
}
type Entries<K, T> = Arc<Mutex<VecDeque<Entry<K, T>>>>;
struct Flights<K, T> {
    entries: Entries<K, T>,
    size: fn(&T) -> usize,
}
impl<K, T> Flights<K, T> {
    fn new(size: fn(&T) -> usize) -> Self {
        Self {
            entries: Arc::default(),
            size,
        }
    }
    fn prune(entries: &mut VecDeque<Entry<K, T>>) {
        let mut bytes = entries
            .iter()
            .map(|entry| entry.flight.bytes.load(Ordering::Acquire))
            .sum::<usize>();
        while bytes > 64 * 1024 * 1024 || entries.len() > 128 {
            let Some(index) = entries
                .iter()
                .position(|entry| !entry.flight.producing.load(Ordering::Acquire))
            else {
                break;
            };
            let entry = entries.remove(index).unwrap();
            bytes = bytes.saturating_sub(entry.flight.bytes.load(Ordering::Acquire));
        }
    }
}
impl<K: Clone + PartialEq + Send + 'static, T: Send + Sync + 'static> Flights<K, T> {
    fn get<F: FnOnce() -> Result<T> + Send + 'static>(
        &self,
        context: &ReadContext,
        key: K,
        fetch: &mut Option<F>,
        rejected: Option<&Arc<T>>,
    ) -> Result<Arc<T>> {
        let demand = Arc::new(context.clone());
        loop {
            if context.is_cancelled() {
                return Err(ReadFailure::cancelled().into());
            }
            let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(index) = entries.iter().position(|e| e.key == key) {
                let entry = entries.remove(index).unwrap();
                let finished = entry
                    .flight
                    .result
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone();
                let producing = entry.flight.producing.load(Ordering::Acquire);
                let replace = rejected.is_some_and(|rejected| {
                    finished.as_ref().is_some_and(|result| {
                        result
                            .as_ref()
                            .is_ok_and(|value| Arc::ptr_eq(rejected, value))
                    })
                });
                if !(replace && !producing) {
                    if finished.as_ref().is_some_and(Result::is_err) && !producing {
                        continue;
                    }
                    let admitted = finished.is_some() || entry.flight.demands.add(&demand);
                    let flight = entry.flight.clone();
                    entries.push_back(entry);
                    drop(entries);
                    let result = flight.wait(context);
                    if !admitted && !context.is_cancelled() {
                        continue;
                    }
                    match result {
                        Err(error) => {
                            let failure = ReadFailure::from_error(error);
                            if !context.is_cancelled() && failure.kind == FailureKind::Cancelled {
                                continue;
                            }
                            return Err(failure.into());
                        }
                        result => return result,
                    }
                }
            }
            let running = entries
                .iter()
                .filter(|e| e.flight.producing.load(Ordering::Acquire))
                .count();
            let foreground = context.priority() == ReadPriority::Selected;
            let non_foreground = entries
                .iter()
                .filter(|entry| {
                    entry.flight.producing.load(Ordering::Acquire) && !entry.flight.foreground
                })
                .count();
            if running >= 8 || (!foreground && non_foreground >= 6) {
                drop(entries);
                std::thread::sleep(Duration::from_millis(5));
                continue;
            }
            let work = fetch
                .take()
                .ok_or_else(|| anyhow!("Shared remote read lost its producer"))?;
            let demands = Arc::new(Demands::default());
            demands.add(&demand);
            let producer = context.producer(demands.clone());
            let flight = Arc::new(Flight {
                result: Mutex::new(None),
                producing: AtomicBool::new(true),
                foreground,
                bytes: AtomicUsize::new(0),
                changed: Condvar::new(),
                demands,
                cancel: producer.cancellation_handle(),
            });
            entries.push_back(Entry {
                key,
                flight: flight.clone(),
            });
            drop(entries);
            let worker = flight.clone();
            let entries = self.entries.clone();
            let size = self.size;
            std::thread::spawn(move || {
                let result =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| producer.run(work)))
                        .unwrap_or_else(|_| Err(anyhow!("Remote read worker stopped")))
                        .map(Arc::new)
                        .map_err(ReadFailure::from_error);
                let bytes = result.as_ref().map(|value| size(value)).unwrap_or(0);
                worker.producing.store(false, Ordering::Release);
                worker.finish(result, bytes, false);
                Self::prune(&mut entries.lock().unwrap_or_else(|e| e.into_inner()));
            });
            return flight.wait(context);
        }
    }
    fn cancel_unneeded(&self) {
        let flights = self
            .entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .map(|entry| entry.flight.clone())
            .collect::<Vec<_>>();
        for flight in flights {
            if flight.producing.load(Ordering::Acquire) && flight.demands.priority().is_none() {
                flight.cancel.cancel();
            }
        }
    }

    fn publish(&self, context: &ReadContext, key: K, value: T) {
        let bytes = (self.size)(&value);
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        let flight = if let Some(index) = entries.iter().position(|e| e.key == key) {
            let entry = entries.remove(index).unwrap();
            let flight = entry.flight.clone();
            entries.push_back(entry);
            flight
        } else {
            let flight = Arc::new(Flight {
                result: Mutex::new(None),
                producing: AtomicBool::new(false),
                foreground: false,
                bytes: AtomicUsize::new(0),
                changed: Condvar::new(),
                demands: Arc::default(),
                cancel: context.cancellation_handle(),
            });
            entries.push_back(Entry {
                key,
                flight: flight.clone(),
            });
            flight
        };
        flight.finish(Ok(Arc::new(value)), bytes, true);
        Self::prune(&mut entries);
        drop(entries);
        if flight.demands.priority().is_some() {
            flight.cancel.cancel();
        }
    }
}

pub(super) struct SessionReads {
    sources: Flights<ContentSource, Vec<u8>>,
    trees: Flights<String, Value>,
}
impl Default for SessionReads {
    fn default() -> Self {
        Self {
            sources: Flights::new(Vec::len),
            trees: Flights::new(|v| v.to_string().len()),
        }
    }
}
impl SessionReads {
    pub fn cancel_unneeded(&self) {
        self.sources.cancel_unneeded();
        self.trees.cancel_unneeded();
    }
    pub fn source(
        &self,
        context: &ReadContext,
        key: ContentSource,
        exact: Option<BlobId>,
        fetch: impl FnOnce() -> Result<Vec<u8>> + Send + 'static,
    ) -> Result<Arc<Vec<u8>>> {
        let mut fetch = Some(fetch);
        let mut rejected = None;
        loop {
            let value = self
                .sources
                .get(context, key.clone(), &mut fetch, rejected.as_ref())?;
            if exact.is_none_or(|id| id.verify(&value).is_ok()) {
                return Ok(value);
            }
            if fetch.is_none() {
                return Err(anyhow!(
                    "Remote bytes do not match their pinned object identity"
                ));
            }
            rejected = Some(value);
        }
    }
    pub fn tree(
        &self,
        context: &ReadContext,
        key: String,
        fetch: impl FnOnce() -> Result<Value> + Send + 'static,
    ) -> Result<Arc<Value>> {
        self.trees.get(context, key, &mut Some(fetch), None)
    }
    pub fn publish(
        &self,
        context: &ReadContext,
        key: ContentSource,
        id: BlobId,
        bytes: Vec<u8>,
    ) -> Result<()> {
        id.verify(&bytes).map_err(|e| anyhow!(e.to_string()))?;
        self.sources.publish(context, key, bytes);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::loading;
    use super::*;
    use std::sync::mpsc;
    use std::time::Instant;

    fn source(path: &str) -> ContentSource {
        ContentSource::RemoteBlob {
            provider: "gitlab".into(),
            host: "fixture".into(),
            repo: "r/p".into(),
            revision: "1".repeat(40),
            path: path.into(),
        }
    }

    #[test]
    fn wrong_provisional_candidate_uses_the_exact_read_without_cache_poisoning() {
        let context = ReadContext::default();
        let key = source("x");
        context
            .source(key.clone(), None, || Ok(b"wrong".to_vec()))
            .unwrap();
        let id = BlobId::from_bytes(b"correct").unwrap();
        let reads = Arc::new(AtomicUsize::new(0));
        let captured = reads.clone();
        assert_eq!(
            &**context
                .source(key.clone(), Some(id), move || {
                    captured.fetch_add(1, Ordering::SeqCst);
                    Ok(b"correct".to_vec())
                })
                .unwrap(),
            b"correct"
        );
        assert_eq!(
            &**context
                .source(key, Some(id), || panic!("verified bytes should be reused"))
                .unwrap(),
            b"correct"
        );
        assert_eq!(reads.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn cancelled_waiter_detaches_while_other_waiter_keeps_producer() {
        let root = ReadContext::default();
        let background = root.prefetch_child();
        let selected = root.child(true);
        let cancelled = background.clone();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            background.source(source("x"), None, move || {
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                assert!(!loading::cancelled());
                Ok(b"shared".to_vec())
            })
        });
        entered_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let (joined_tx, joined_rx) = mpsc::channel();
        let waiter = std::thread::spawn(move || {
            joined_tx.send(()).unwrap();
            selected.source(source("x"), None, || panic!("duplicate shared read"))
        });
        joined_rx.recv().unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        while root
            .reads
            .sources
            .entries
            .lock()
            .unwrap()
            .front()
            .unwrap()
            .flight
            .demands
            .priority()
            != Some(ReadPriority::Selected)
        {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        let start = Instant::now();
        cancelled.cancel();
        assert!(worker.join().unwrap().is_err());
        assert!(start.elapsed() < Duration::from_millis(250));
        release_tx.send(()).unwrap();
        assert_eq!(&**waiter.join().unwrap().unwrap(), b"shared");
    }

    #[test]
    fn selected_demand_reaches_a_background_owned_tree_dependency() {
        let root = ReadContext::default();
        let background = root.prefetch_child();
        let selected = root.child(true);
        let (entered_tx, entered_rx) = mpsc::channel();
        let (priority_tx, priority_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            background.source(source("x"), None, move || {
                loading::tree_page("tree".into(), move || {
                    entered_tx
                        .send(loading::current().priority() == ReadPriority::Prefetch)
                        .unwrap();
                    let deadline = Instant::now() + Duration::from_secs(2);
                    while loading::current().priority() != ReadPriority::Selected {
                        assert!(Instant::now() < deadline);
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    priority_tx.send(()).unwrap();
                    Ok(serde_json::json!({"tree":[]}))
                })?;
                Ok(b"same".to_vec())
            })
        });
        assert!(entered_rx.recv_timeout(Duration::from_secs(1)).unwrap());
        let waiter =
            std::thread::spawn(move || selected.source(source("x"), None, || panic!("duplicate")));
        priority_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(&**waiter.join().unwrap().unwrap(), b"same");
        assert_eq!(&**worker.join().unwrap().unwrap(), b"same");
    }

    #[test]
    fn verified_publication_satisfies_waiters_and_rejects_late_raw_bytes() {
        let root = ReadContext::default();
        let reader = root.prefetch_child();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (ended_tx, ended_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            reader.source(source("x"), None, move || {
                entered_tx.send(()).unwrap();
                let deadline = Instant::now() + Duration::from_secs(2);
                while !loading::cancelled() {
                    assert!(Instant::now() < deadline);
                    std::thread::sleep(Duration::from_millis(1));
                }
                ended_tx.send(()).unwrap();
                Ok(b"late counterfeit".to_vec())
            })
        });
        entered_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let id = BlobId::from_bytes(b"proved").unwrap();
        root.publish(source("x"), id, b"proved".to_vec()).unwrap();
        assert_eq!(&**worker.join().unwrap().unwrap(), b"proved");
        ended_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(
            &**root
                .source(source("x"), Some(id), || panic!("duplicate after proof"))
                .unwrap(),
            b"proved"
        );
    }

    #[test]
    fn a_new_waiter_waits_for_a_closing_producer_before_starting_its_successor() {
        let root = ReadContext::default();
        let selected = root.child(true);
        let cancel = selected.clone();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (closing_tx, closing_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            selected.source(source("closing"), None, move || {
                entered_tx.send(()).unwrap();
                while !loading::cancelled() {
                    std::thread::sleep(Duration::from_millis(1));
                }
                closing_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                assert!(loading::cancelled());
                Err(ReadFailure::cancelled().into())
            })
        });
        entered_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        cancel.cancel();
        assert!(worker.join().unwrap().is_err());
        closing_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let next = root.child(true);
        let (next_tx, next_rx) = mpsc::channel();
        let successor = std::thread::spawn(move || {
            next.source(source("closing"), None, move || {
                next_tx.send(()).unwrap();
                Ok(b"successor".to_vec())
            })
        });
        assert!(next_rx.recv_timeout(Duration::from_millis(40)).is_err());
        release_tx.send(()).unwrap();
        next_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(&**successor.join().unwrap().unwrap(), b"successor");
    }

    #[test]
    fn failed_producer_unwinds_release_waiters_and_allow_a_later_explicit_read() {
        let context = ReadContext::default();
        assert!(context
            .source(source("panic"), None, || -> Result<Vec<u8>> {
                panic!("controlled producer failure")
            })
            .is_err());
        assert_eq!(
            &**context
                .source(source("panic"), None, || Ok(b"recovered".to_vec()))
                .unwrap(),
            b"recovered"
        );
    }

    #[test]
    fn selected_producers_have_reserved_capacity_while_nonselected_reads_are_blocked() {
        let root = ReadContext::default();
        let release = Arc::new(AtomicBool::new(false));
        let (entered_tx, entered_rx) = mpsc::channel();
        let mut workers = Vec::new();
        for index in 0..7 {
            let context = root.child(false);
            let release = release.clone();
            let entered = entered_tx.clone();
            workers.push(std::thread::spawn(move || {
                context.source(source(&format!("background{index}")), None, move || {
                    entered.send(false).unwrap();
                    let deadline = Instant::now() + Duration::from_secs(3);
                    while !release.load(Ordering::Acquire) {
                        assert!(Instant::now() < deadline);
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    Ok(b"background".to_vec())
                })
            }));
        }
        for _ in 0..6 {
            assert!(!entered_rx.recv_timeout(Duration::from_secs(1)).unwrap());
        }
        assert!(entered_rx.recv_timeout(Duration::from_millis(30)).is_err());
        let selected = root.child(true);
        let entered = entered_tx.clone();
        let foreground = std::thread::spawn(move || {
            selected.source(source("selected"), None, move || {
                entered.send(true).unwrap();
                Ok(b"selected".to_vec())
            })
        });
        assert!(entered_rx.recv_timeout(Duration::from_secs(1)).unwrap());
        assert_eq!(&**foreground.join().unwrap().unwrap(), b"selected");
        release.store(true, Ordering::Release);
        for worker in workers {
            assert!(worker.join().unwrap().is_ok());
        }
    }

    #[test]
    fn completed_candidates_have_both_entry_and_byte_bounds() {
        let context = ReadContext::default();
        let reads = Flights::new(Vec::len);
        for index in 0..140 {
            reads.publish(&context, index, Vec::new());
        }
        assert!(reads.entries.lock().unwrap().len() <= 128);
        for index in 140..145 {
            reads.publish(&context, index, vec![1u8; 18 * 1024 * 1024]);
        }
        let entries = reads.entries.lock().unwrap();
        let bytes = entries
            .iter()
            .map(|e| {
                e.flight
                    .result
                    .lock()
                    .unwrap()
                    .as_ref()
                    .unwrap()
                    .as_ref()
                    .unwrap()
                    .len()
            })
            .sum::<usize>();
        assert!(bytes <= 64 * 1024 * 1024);
    }
}
