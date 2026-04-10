mod api;
pub(crate) mod auth;
mod control;

pub use api::{console_dashboard_data_handler, dashboard_data_handler};
pub use auth::{auth_check, console_page, dashboard_page, login};
pub use control::{do_upgrade, get_version, restart_pool, stop_pool};
