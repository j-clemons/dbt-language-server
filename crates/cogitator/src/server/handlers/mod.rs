//! Pure message handlers (05 §6.2): `fn(&Snapshot, …) -> Reply` — every
//! effectful or offloadable handler lives here, producing ordered
//! effects plus a response that the shell transmits. Sub-millisecond
//! query handlers stay on the dispatcher's generic decode→feature→send
//! path; their logic is already pure in `cogitator_analysis::features`.

pub mod commands;
pub mod doctor;
pub mod lineage;
pub mod navigation;
pub mod plan;
mod plan_compiled;
pub mod profiles;
