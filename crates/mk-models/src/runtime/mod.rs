//! BC-12 local model runtime (M5c, ADR-0015, domain §7.8, INV-MM-05/06): find the
//! user-installed `llama-server` and its backend ([`find`]), run one process per loaded model on
//! `127.0.0.1` with a random port and key ([`process`]), and supervise it ([`Supervisor`]):
//! start on demand, health check, crash restarts, idle stop, stop at quit.

pub mod find;
pub mod process;
pub mod state;
mod supervisor;

pub use find::{Backend, BinaryInfo, Build, Device, FindError, INSTALL_GUIDE, MIN_BUILD, Source};
pub use process::{Failure, ModelSpec, SpawnOptions};
pub use state::{MAX_RESTARTS, RuntimeState};
pub use supervisor::{
    HEALTH_TIMEOUT, IDLE_STOP, Lease, RuntimeConfig, STOP_GRACE, SlotStatus, Supervisor,
};
