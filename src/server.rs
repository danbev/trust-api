use actix_cors::Cors;
use actix_web::web::Data;
use actix_web::{middleware::Logger, App, HttpServer};
use std::sync::Arc;
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

use crate::guac;
use crate::index;
use crate::package;
use crate::sbom::SbomRegistry;
use crate::vulnerability;
use crate::Snyk;

pub struct Server {
    bind: String,
    port: u16,
    guac_url: String,
    snyk: Snyk,
}

#[derive(OpenApi)]
#[openapi(
        paths(
            package::get_package,
            package::query_package,
            package::query_package_dependencies,
            package::query_package_dependents,
            package::query_package_versions,
            vulnerability::query_vulnerability,
        ),
        components(
            schemas(package::Package, package::PackageList, package::PackageDependencies, package::PackageDependents, package::PackageRef, package::SnykData, package::VulnerabilityRef, vulnerability::Vulnerability)
        ),
        tags(
            (name = "package", description = "Package query endpoints."),
            (name = "vulnerability", description = "Vulnerability query endpoints")
        ),
    )]
pub struct ApiDoc;

impl Server {
    pub fn new(bind: String, port: u16, guac_url: String, snyk: Snyk) -> Self {
        Self {
            bind,
            port,
            guac_url,
            snyk,
        }
    }

    pub async fn run(self) -> anyhow::Result<()> {
        let openapi = ApiDoc::openapi();

        let sboms = Arc::new(SbomRegistry::new());
        let guac = Arc::new(guac::Guac::new(&self.guac_url, sboms.clone()));

        HttpServer::new(move || {
            let cors = Cors::default()
                .send_wildcard()
                .allow_any_origin()
                .allow_any_method()
                .allow_any_header()
                .max_age(3600);

            App::new()
                .wrap(Logger::default())
                .wrap(cors)
                .app_data(Data::new(sboms.clone()))
                .app_data(Data::new(package::TrustedContent::new(
                    guac.clone(),
                    sboms.clone(),
                    self.snyk.clone(),
                )))
                .app_data(Data::new(guac.clone()))
                .configure(package::configure())
                .configure(vulnerability::configure())
                .configure(index::configure())
                .service(SwaggerUi::new("/swagger-ui/{_:.*}").url("/openapi.json", openapi.clone()))
        })
        .bind((self.bind, self.port))?
        .run()
        .await?;
        Ok(())
    }
}
