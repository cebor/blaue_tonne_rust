//! OpenAPI specification (served as Swagger UI under `/docs`).

use utoipa::OpenApi;

use crate::handlers;

#[derive(OpenApi)]
#[openapi(
    paths(
        handlers::health_check,
        handlers::lk_rosenheim_handler,
    ),
    components(
        schemas(
            handlers::HealthResponse,
            handlers::ErrorDetail,
            handlers::DistrictQuery,
        )
    ),
    info(
        title = "Blaue Tonne API",
        // version intentionally omitted: utoipa defaults it to CARGO_PKG_VERSION,
        // keeping the spec in sync with Cargo.toml
        description = "Altpapier (Blaue Tonne) collection dates for Landkreis Rosenheim",
        contact(
            name = "Source Code",
            url = "https://gitlab.stkn.org/felix/blaue_tonne_rust"
        ),
        license(
            name = "MIT",
            identifier = "MIT"
        )
    )
)]
pub struct ApiDoc;
