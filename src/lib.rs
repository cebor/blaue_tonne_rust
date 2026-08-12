// Modules the integration tests reach directly are `pub`; the rest is
// `pub(crate)` and surfaces through the re-exports below.
pub mod cache;
pub mod config;
pub mod errors;
pub mod index;
pub mod middleware;
pub mod pdf_parser;

pub(crate) mod download;
pub(crate) mod handlers;
pub(crate) mod openapi;
pub(crate) mod router;
pub(crate) mod state;

pub use middleware::ResolvedClientIp;
pub use router::build_router;
pub use state::AppState;
