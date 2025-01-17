use crate::package::Package;
use crate::package::PackageDependencies;
use crate::package::PackageRef;
use crate::package::VulnerabilityRef;
use crate::sbom::SbomRegistry;
use crate::vulnerability::Cvss3;
use crate::vulnerability::Vulnerability;
use anyhow::anyhow;
use chrono::DateTime;
use chrono::Utc;
use guac_rs::client::GuacClient;
use http::StatusCode;
use packageurl::PackageUrl;
use std::sync::Arc;

#[derive(Clone)]
pub struct Guac {
    client: GuacClient,
    sbom: Arc<SbomRegistry>,
}

impl Guac {
    pub fn new(url: &str, sbom: Arc<SbomRegistry>) -> Self {
        let client = GuacClient::new(url.to_string());
        Self { client, sbom }
    }

    pub async fn get_packages(
        &self,
        purl: PackageUrl<'_>,
    ) -> Result<Vec<PackageRef>, anyhow::Error> {
        let pkgs = self
            .client
            .get_packages(&purl.to_string())
            .await
            .map_err(|e| {
                let e = format!("Error getting packages from GUAC: {:?}", e);
                log::warn!("{}", e);
                anyhow!(e)
            })?;
        let mut ret = Vec::new();
        for pkg in pkgs.iter() {
            let t = &pkg.type_;
            for namespace in pkg.namespaces.iter() {
                for name in namespace.names.iter() {
                    for version in name.versions.iter() {
                        let purl = format!(
                            "pkg:{}/{}/{}@{}",
                            t, namespace.namespace, name.name, version.version
                        );
                        let p = PackageRef {
                            purl: purl.clone(),
                            href: format!("/api/package?purl={}", &urlencoding::encode(&purl)),
                            trusted: Some(namespace.namespace == "redhat"),
                            sbom: if self.sbom.exists(&purl) {
                                Some(format!(
                                    "/api/package/sbom?purl={}",
                                    &urlencoding::encode(&purl)
                                ))
                            } else {
                                None
                            },
                        };
                        ret.push(p);
                    }
                }
            }
        }
        Ok(ret)
    }

    pub async fn get_vulnerability(&self, cve_id: &str) -> Result<Vulnerability, anyhow::Error> {
        log::info!("Lookup cve {}", cve_id);
        let vulns = self.client.get_vulnerabilities(cve_id).await.map_err(|e| {
            let e = format!("Error getting vulnerabilities from GUAC: {:?}", e);
            log::warn!("{}", e);
            anyhow!(e)
        })?;

        let mut packages = Vec::new();
        for vuln in vulns.iter() {
            let pkg = &vuln.package;
            let t = &pkg.type_;
            for namespace in pkg.namespaces.iter() {
                for name in namespace.names.iter() {
                    for version in name.versions.iter() {
                        let purl = format!(
                            "pkg:{}/{}/{}@{}",
                            t, namespace.namespace, name.name, version.version
                        );
                        let p = PackageRef {
                            purl: purl.clone(),
                            href: format!("/api/package?purl={}", &urlencoding::encode(&purl)),
                            trusted: Some(namespace.namespace == "redhat"),
                            sbom: if self.sbom.exists(&purl) {
                                Some(format!(
                                    "/api/package/sbom?purl={}",
                                    &urlencoding::encode(&purl)
                                ))
                            } else {
                                None
                            },
                        };
                        packages.push(p);
                    }
                }
            }
        }

        // Fetch CVE details to get summary for this vulnerability.
        let hydra = format!(
            "https://access.redhat.com/hydra/rest/securitydata/cve/{}.json",
            cve_id.to_ascii_uppercase()
        );
        let response = reqwest::get(hydra).await;
        let mut summary = "Unavailable".to_string();
        let mut severity = None;
        let mut cvss3 = None;
        let mut date = None;
        if let Ok(response) = response {
            if response.status() == StatusCode::OK {
                if let Ok(data) = response.json::<serde_json::Value>().await {
                    if let Some(Some(details)) = data.get("details").map(|s| s.as_array()) {
                        if let Some(Some(details)) = details.get(0).map(|s| s.as_str()) {
                            summary = details.to_string();
                        }
                    }
                    if let Some(Some(data)) = data.get("cvss3").map(|s| s.as_object()) {
                        match (
                            data.get("cvss3_base_score").map(|s| s.as_str()),
                            data.get("status").map(|s| s.as_str()),
                        ) {
                            (Some(Some(score)), Some(Some(status))) => {
                                cvss3.replace(Cvss3 {
                                    score: score.to_string(),
                                    status: status.to_string(),
                                });
                            }
                            _ => {}
                        }
                    }
                    if let Some(Some(data)) = data.get("threat_severity").map(|s| s.as_str()) {
                        severity.replace(data.to_string());
                    }

                    if let Some(Some(data)) = data.get("public_date").map(|s| s.as_str()) {
                        if let Ok(d) = data.parse::<DateTime<Utc>>() {
                            date.replace(d);
                        }
                    }
                }
            }
        }

        Ok(Vulnerability {
            cve: cve_id.to_string(),
            summary,
            severity,
            cvss3,
            date,
            // TODO: Avoid hardcoding url, get from guac
            advisory: format!(
                "https://access.redhat.com/security/cve/{}",
                cve_id.to_lowercase()
            ),
            packages,
        })
    }

    pub async fn get_vulnerabilities(
        &self,
        purl: &str,
    ) -> Result<Vec<VulnerabilityRef>, anyhow::Error> {
        let vulns = self.client.certify_vuln(purl).await.map_err(|e| {
            let e = format!("Error getting vulnerabilities from GUAC: {:?}", e);
            log::warn!("{}", e);
            anyhow!(e)
        })?;

        let mut ret = Vec::new();
        for vuln in vulns.iter() {
            match &vuln.vulnerability {
                guac_rs::vuln::certify_vuln_q1::AllCertifyVulnTreeVulnerability::OSV(osv) => {
                    let id = osv.osv_id.clone();
                    let vuln_ref = VulnerabilityRef {
                        cve: id.clone(),
                        href: format!(
                            "{}/{}",
                            "https://osv.dev/vulnerability",
                            id.replace("ghsa", "GHSA")
                        ), //TODO fix guac id format
                    };
                    //TODO fix guac repeated entries
                    if !ret.contains(&vuln_ref) {
                        ret.push(vuln_ref);
                    }
                }
                guac_rs::vuln::certify_vuln_q1::AllCertifyVulnTreeVulnerability::CVE(id) => {
                    let vuln_ref = VulnerabilityRef {
                        cve: id.cve_id.clone(),
                        href: format!(
                            "https://access.redhat.com/security/cve/{}",
                            id.cve_id.to_lowercase()
                        ), //TODO fix guac id format
                    };
                    //TODO fix guac repeated entries
                    if !ret.contains(&vuln_ref) {
                        ret.push(vuln_ref);
                    }
                }
                _ => {}
            };
        }
        Ok(ret)
    }

    pub async fn get_dependencies(&self, purl: &str) -> Result<PackageDependencies, anyhow::Error> {
        let deps = self.client.get_dependencies(purl).await.map_err(|e| {
            let e = format!("Error getting dependencies from GUAC: {:?}", e);
            log::warn!("{}", e);
            anyhow!(e)
        })?;

        let mut ret = Vec::new();
        for dep in deps.iter() {
            let pkg = &dep.dependent_package;
            let t = &pkg.type_;
            for namespace in pkg.namespaces.iter() {
                for name in namespace.names.iter() {
                    for version in name.versions.iter() {
                        let purl = format!(
                            "pkg:{}/{}/{}@{}",
                            t, namespace.namespace, name.name, version.version
                        );
                        let p = PackageRef {
                            purl: purl.clone(),
                            href: format!("/api/package?purl={}", &urlencoding::encode(&purl)),
                            trusted: None,
                            sbom: if self.sbom.exists(&purl) {
                                Some(format!(
                                    "/api/package/sbom?purl={}",
                                    &urlencoding::encode(&purl)
                                ))
                            } else {
                                None
                            },
                        };
                        //TODO fix guac repeated entries
                        if !ret.contains(&p) {
                            ret.push(p);
                        }
                    }
                }
            }
        }
        Ok(PackageDependencies(ret))
    }

    pub async fn get_all_packages(&self) -> Result<Vec<Package>, anyhow::Error> {
        let all_packages = self.client.get_all_packages().await?;

        let mut all = Vec::new();
        for pkg in all_packages.iter() {
            let t = &pkg.type_;
            for namespace in pkg.namespaces.iter() {
                for name in namespace.names.iter() {
                    for version in name.versions.iter() {
                        let purl = format!(
                            "pkg:{}/{}/{}@{}",
                            t, namespace.namespace, name.name, version.version
                        );
                        let vulns = self.get_vulnerabilities(&purl).await?;
                        let p = Package {
                            purl: Some(purl.to_string()),
                            href: Some(format!(
                                "/api/package?purl={}",
                                &urlencoding::encode(&purl.to_string())
                            )),
                            trusted: Some(namespace.namespace == "redhat"),
                            trusted_versions: vec![],
                            snyk: None,
                            vulnerabilities: vulns,
                            sbom: if self.sbom.exists(&purl) {
                                Some(format!(
                                    "/api/package/sbom?purl={}",
                                    &urlencoding::encode(&purl)
                                ))
                            } else {
                                None
                            },
                        };
                        all.push(p);
                    }
                }
            }
        }
        Ok(all)
    }

    pub async fn get_dependents(&self, purl: &str) -> Result<PackageDependencies, anyhow::Error> {
        let deps = self.client.is_dependent(purl).await.map_err(|e| {
            let e = format!("Error getting dependents from GUAC: {:?}", e);
            log::warn!("{}", e);
            anyhow!(e)
        })?;

        let mut ret = Vec::new();
        for dep in deps.iter() {
            let pkg = &dep.package;
            let t = &pkg.type_;
            for namespace in pkg.namespaces.iter() {
                for name in namespace.names.iter() {
                    for version in name.versions.iter() {
                        let purl = format!(
                            "pkg:{}/{}/{}@{}",
                            t, namespace.namespace, name.name, version.version
                        );
                        let p = PackageRef {
                            purl: purl.clone(),
                            href: format!("/api/package?purl={}", &urlencoding::encode(&purl)),
                            trusted: None,
                            sbom: if self.sbom.exists(&purl) {
                                Some(format!(
                                    "/api/package/sbom?purl={}",
                                    &urlencoding::encode(&purl)
                                ))
                            } else {
                                None
                            },
                        };
                        ret.push(p);
                    }
                }
            }
        }
        Ok(PackageDependencies(ret))
    }
}
