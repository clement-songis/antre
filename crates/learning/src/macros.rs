//! Logging shims so call sites stay readable with the `tracing` feature off.

macro_rules! log_warn {
    ($($arg:tt)*) => {{
        #[cfg(feature = "tracing")]
        tracing::warn!($($arg)*);
    }};
}

macro_rules! log_debug {
    ($($arg:tt)*) => {{
        #[cfg(feature = "tracing")]
        tracing::debug!($($arg)*);
    }};
}

macro_rules! log_info {
    ($($arg:tt)*) => {{
        #[cfg(feature = "tracing")]
        tracing::info!($($arg)*);
    }};
}

pub(crate) use {log_debug, log_info, log_warn};
