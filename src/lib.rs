// Modules reached directly by the integration tests stay `pub`; everything the
// tests only need indirectly is `pub(crate)` and surfaces through the re-exports
// below, so there is exactly one public path per item.
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
