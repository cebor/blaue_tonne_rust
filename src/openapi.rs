//! OpenAPI specification (served as Swagger UI under `/docs`).

use utoipa::OpenApi;
use utoipa::openapi::schema::{ObjectBuilder, Type};

use crate::{errors, handlers};

#[derive(OpenApi)]
#[openapi(
    paths(
        handlers::health_check,
        handlers::lk_rosenheim_handler,
    ),
    components(
        schemas(
            handlers::HealthResponse,
            errors::ErrorDetail,
            handlers::DistrictQuery,
        )
    ),
    info(
        title = "Blaue Tonne API",
        // version omitted: utoipa defaults it to CARGO_PKG_VERSION
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

impl ApiDoc {
    /// The spec with `district` narrowed to the districts actually indexed, so
    /// Swagger UI's "Try it out" offers them as a dropdown instead of a free
    /// text field.
    ///
    /// The list is the index's, built from the plans at startup — not a constant
    /// that could fall out of step with them. A name that reaches this schema is
    /// one the service can answer. The parameter's description and everything
    /// else about the operation are left alone.
    pub fn with_districts(districts: Vec<String>) -> utoipa::openapi::OpenApi {
        let mut doc = Self::openapi();

        // An empty index cannot start a server (`build_index` fails first), so
        // this only guards a doc built by hand: an empty `enum` would be a
        // parameter no value satisfies.
        if districts.is_empty() {
            return doc;
        }

        if let Some(parameter) = doc
            .paths
            .paths
            .get_mut("/lk_rosenheim")
            .and_then(|item| item.get.as_mut())
            .and_then(|operation| operation.parameters.as_mut())
            .and_then(|parameters| parameters.iter_mut().find(|p| p.name == "district"))
        {
            parameter.schema = Some(
                ObjectBuilder::new()
                    .schema_type(Type::String)
                    .enum_values(Some(districts))
                    .into(),
            );
        }

        doc
    }
}
