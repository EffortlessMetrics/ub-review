use std::sync::{Mutex, MutexGuard, OnceLock};

use anyhow::Result;

pub fn cli_subprocess_test_lock() -> Result<MutexGuard<'static, ()>> {
    static CLI_SUBPROCESS_TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    // Recover a poisoned lock instead of erroring: one failing test must
    // produce one failure receipt, not cascade into every later subprocess
    // test in the suite.
    Ok(CLI_SUBPROCESS_TEST_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner))
}
