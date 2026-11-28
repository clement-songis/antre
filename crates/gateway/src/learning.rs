//! Gateway side of the learning loop: starts the service, schedules reviews
//! after runs, runs the curator on idle, and adapts the provider registry to
//! the review model. The loop itself lives in `moltis-learning`.

mod history;
mod model;
mod runtime;

pub(crate) use runtime::{
    announce, on_run_finished, on_run_started, reserved_skill_names, review_now, service,
    skill_activity, start,
};
