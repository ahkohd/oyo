use crate::app::App;
use oyo_core::MultiFileDiff;
use std::ops::{Deref, DerefMut};
use std::sync::{Mutex, MutexGuard};

pub(crate) const DEFAULT_DIFF_MAX_BYTES: u64 = 16 * 1024 * 1024;

static DIFF_SETTINGS_LOCK: Mutex<()> = Mutex::new(());

pub(crate) struct DiffSettingsGuard {
    _lock: MutexGuard<'static, ()>,
}

impl DiffSettingsGuard {
    pub(crate) fn new(diff_max_bytes: u64) -> Self {
        let lock = DIFF_SETTINGS_LOCK.lock().unwrap();
        MultiFileDiff::set_diff_max_bytes(diff_max_bytes);
        MultiFileDiff::set_diff_defer(true);
        Self { _lock: lock }
    }
}

impl Default for DiffSettingsGuard {
    fn default() -> Self {
        Self::new(DEFAULT_DIFF_MAX_BYTES)
    }
}

impl Drop for DiffSettingsGuard {
    fn drop(&mut self) {
        MultiFileDiff::set_diff_max_bytes(DEFAULT_DIFF_MAX_BYTES);
        MultiFileDiff::set_diff_defer(true);
    }
}

pub(crate) struct TestApp {
    _guard: DiffSettingsGuard,
    app: App,
}

impl TestApp {
    pub(crate) fn new_with_guard<F>(diff_max_bytes: u64, builder: F) -> Self
    where
        F: FnOnce() -> App,
    {
        let guard = DiffSettingsGuard::new(diff_max_bytes);
        let app = builder();
        Self { _guard: guard, app }
    }

    pub(crate) fn new_default<F>(builder: F) -> Self
    where
        F: FnOnce() -> App,
    {
        Self::new_with_guard(DEFAULT_DIFF_MAX_BYTES, builder)
    }
}

impl Deref for TestApp {
    type Target = App;

    fn deref(&self) -> &Self::Target {
        &self.app
    }
}

impl DerefMut for TestApp {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.app
    }
}
