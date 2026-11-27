use tokio::sync::Semaphore;

/// Serializes tests that override the process-wide data directory, sync or
/// async, so one test never reads another's override.
pub(crate) static DATA_DIR_LOCK: Semaphore = Semaphore::const_new(1);
