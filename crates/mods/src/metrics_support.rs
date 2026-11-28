//! Metric helpers that compile to nothing without the `metrics` feature.

#[cfg(feature = "metrics")]
use antre_metrics::{counter, gauge, histogram, mods};

pub(crate) fn install_outcome(outcome: &'static str) {
    #[cfg(feature = "metrics")]
    counter!(mods::INSTALLS_TOTAL, "outcome" => outcome).increment(1);
    #[cfg(not(feature = "metrics"))]
    let _ = outcome;
}

pub(crate) fn installed(count: usize) {
    #[cfg(feature = "metrics")]
    gauge!(mods::INSTALLED).set(count as f64);
    #[cfg(not(feature = "metrics"))]
    let _ = count;
}

pub(crate) fn call(mod_id: &str, kind: &'static str, seconds: f64, failed: bool) {
    #[cfg(feature = "metrics")]
    {
        let mod_id = mod_id.to_string();
        counter!(mods::CALLS_TOTAL, "mod" => mod_id.clone(), "kind" => kind).increment(1);
        histogram!(mods::CALL_DURATION_SECONDS, "kind" => kind).record(seconds);
        if failed {
            counter!(mods::CALL_ERRORS_TOTAL, "mod" => mod_id, "kind" => kind).increment(1);
        }
    }
    #[cfg(not(feature = "metrics"))]
    let _ = (mod_id, kind, seconds, failed);
}

pub(crate) fn denial(mod_id: &str, capability: &'static str) {
    #[cfg(feature = "metrics")]
    counter!(
        mods::CAPABILITY_DENIALS_TOTAL,
        "mod" => mod_id.to_string(),
        "capability" => capability
    )
    .increment(1);
    #[cfg(not(feature = "metrics"))]
    let _ = (mod_id, capability);
}
