//! LFS negotiation for standalone transfers and native Xet downloads.
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::Arc;

use http::{HeaderMap, StatusCode};
use reqwest::Url;
use reqwest_middleware::ClientWithMiddleware;
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use xet_client::cas_client::auth::DirectRefreshRouteTokenRefresher;
use xet_client::common::http_client::build_http_client;
use xet_client::hub_client::{CasJWTInfo, CredentialHelper, Operation};
use xet_pkg::legacy::XetFileInfo;
use xet_pkg::legacy::data_client::download_async;
use xet_runtime::core::XetContext;

use super::xet_agent::XetProgressUpdaterWrapper;
use crate::auth::get_credential;
use crate::errors::{GitXetError, Result};
use crate::git_repo::GitRepo;
use crate::git_url::{GitUrl, Scheme};
use crate::lfs_agent_protocol::{GitBatchApiResponseAction, ProgressUpdater, TransferRequest};

pub(super) fn remote_from_lfs_url(url: &str) -> Result<GitUrl> {
    let remote: GitUrl = url
        .trim_end_matches('/')
        .strip_suffix("/info/lfs")
        .ok_or_else(|| GitXetError::config_error("--lfs-url must end in /info/lfs"))?
        .parse()?;
    if !matches!(remote.scheme(), Scheme::Http | Scheme::Https) {
        return Err(GitXetError::config_error("--lfs-url must use HTTP or HTTPS"));
    }
    Ok(remote)
}

pub(super) struct LfsClient {
    ctx: XetContext,
    repo: GitRepo,
    remote: GitUrl,
    endpoint: String,
    client: ClientWithMiddleware,
    credential: Option<Arc<dyn CredentialHelper>>,
    token: Option<(Arc<DirectRefreshRouteTokenRefresher>, CasJWTInfo)>,
}

#[derive(Deserialize)]
struct BatchResponse {
    transfer: Option<String>,
    objects: Vec<BatchObject>,
}

#[derive(Deserialize)]
struct BatchObject {
    oid: String,
    size: u64,
    error: Option<serde_json::Value>,
    #[serde(default)]
    actions: HashMap<String, GitBatchApiResponseAction>,
}

impl LfsClient {
    pub fn token_refresher(&self, route: &str) -> Arc<DirectRefreshRouteTokenRefresher> {
        Arc::new(DirectRefreshRouteTokenRefresher::new(
            self.ctx.clone(),
            route,
            self.client.clone(),
            self.credential.clone(),
        ))
    }

    pub fn new(ctx: &XetContext, repo: GitRepo, remote: GitUrl, endpoint: Option<String>) -> Result<Self> {
        let endpoint = endpoint.unwrap_or(remote.to_default_lfs_endpoint()?);
        let mut headers = HeaderMap::new();
        headers.insert(http::header::USER_AGENT, concat!("git-xet/", env!("CARGO_PKG_VERSION")).parse().unwrap());
        Ok(Self {
            ctx: ctx.clone(),
            repo,
            remote,
            endpoint,
            client: build_http_client(ctx, "", None, Some(Arc::new(headers)))?,
            credential: None,
            token: None,
        })
    }

    pub async fn batch(
        &mut self,
        req: &TransferRequest,
        operation: Operation,
    ) -> Result<Option<GitBatchApiResponseAction>> {
        let body = json!({
            "operation": operation.as_str(),
            "transfers": if matches!(operation, Operation::Upload) { vec!["xet"] } else { vec!["basic"] },
            "objects": [{"oid": req.oid, "size": req.size}],
        });
        // Try public access first. Only invoke Git's credential helper when the server requests it.
        for attempt in 0..2 {
            let mut request = self
                .client
                .post(format!("{}/objects/batch", self.endpoint.trim_end_matches('/')))
                .header(http::header::ACCEPT, "application/vnd.git-lfs+json")
                .header(http::header::CONTENT_TYPE, "application/vnd.git-lfs+json")
                .body(body.to_string());
            if let Some(credential) = &self.credential {
                request = credential.fill_credential(request).await.map_err(GitXetError::internal)?;
            }
            let response = request
                .send()
                .await
                .map_err(|_| GitXetError::internal("LFS batch request failed"))?;
            if response.status() == StatusCode::UNAUTHORIZED && attempt == 0 {
                self.credential = Some(get_credential(&self.repo, &self.remote, operation)?);
                continue;
            }
            let response = response.error_for_status().map_err(http_error)?;
            let batch: BatchResponse = response.json().await.map_err(http_error)?;
            if matches!(operation, Operation::Download) && !matches!(batch.transfer.as_deref(), None | Some("basic")) {
                return Err(GitXetError::not_supported("Unexpected LFS download transfer type"));
            }
            let mut object = batch
                .objects
                .into_iter()
                .find(|o| o.oid == req.oid)
                .ok_or_else(|| GitXetError::internal("LFS response omitted the requested object"))?;
            if object.error.is_some() || object.size != req.size {
                return Err(GitXetError::internal("LFS object unavailable or size mismatch"));
            }
            let action = object.actions.remove(operation.as_str());
            if action.is_some() && matches!(operation, Operation::Upload) && batch.transfer.as_deref() != Some("xet") {
                return Err(GitXetError::not_supported("Server did not select Xet uploads"));
            }
            return Ok(action);
        }
        unreachable!()
    }

    async fn read_token(&mut self) -> Result<&(Arc<DirectRefreshRouteTokenRefresher>, CasJWTInfo)> {
        if self.token.is_none() {
            let info = self.remote.repo_info()?;
            let route = format!(
                "{}/api/{}s/{}/xet-read-token/main",
                self.remote.to_derived_http_host_url()?,
                info.repo_type,
                info.full_name
            );
            for attempt in 0..2 {
                let refresher = Arc::new(DirectRefreshRouteTokenRefresher::new(
                    self.ctx.clone(),
                    &route,
                    self.client.clone(),
                    self.credential.clone(),
                ));
                match refresher.get_cas_jwt().await {
                    Ok(token) => {
                        self.token = Some((refresher, token));
                        break;
                    },
                    Err(error) if error.status() == Some(StatusCode::UNAUTHORIZED) && attempt == 0 => {
                        self.credential = Some(get_credential(&self.repo, &self.remote, Operation::Download)?);
                    },
                    Err(error) => return Err(error.into()),
                }
            }
        }
        Ok(self.token.as_ref().unwrap())
    }

    pub async fn download<W: Write + Send + Sync + 'static>(
        &mut self,
        req: &TransferRequest,
        path: &Path,
        progress: ProgressUpdater<W>,
    ) -> Result<()> {
        let action = if req.action.href.is_empty() {
            self.batch(req, Operation::Download)
                .await?
                .ok_or_else(|| GitXetError::internal("LFS response omitted the download action"))?
        } else {
            req.action.clone()
        };

        if let Some(hash) = bridge_hash(&action.href) {
            tracing::info!(oid = %req.oid, "Downloading LFS object using native Xet");
            let (refresher, token) = self.read_token().await?;
            let (refresher, endpoint, token_info) =
                (refresher.clone(), token.cas_url.clone(), (token.access_token.clone(), token.exp));
            let progress = Arc::new(XetProgressUpdaterWrapper { updater: progress });
            download_async(
                &self.ctx,
                vec![(XetFileInfo::new(hash, req.size), path.to_string_lossy().into_owned())],
                Some(endpoint),
                Some(token_info),
                Some(refresher),
                Some(vec![progress]),
                None,
            )
            .await
            .map_err(|_| GitXetError::internal("Xet download failed; use --verbose --log for details"))?;
        } else {
            // Objects that have not been converted to Xet still have ordinary LFS download actions.
            let mut request = self.client.get(&action.href);
            for (name, value) in &action.header {
                request = request.header(name, value);
            }
            let mut response = request
                .send()
                .await
                .map_err(|_| GitXetError::internal("LFS download request failed"))?
                .error_for_status()
                .map_err(http_error)?;
            let mut file = std::fs::File::create(path)?;
            let mut written = 0;
            while let Some(chunk) = response.chunk().await.map_err(http_error)? {
                written += chunk.len() as u64;
                if written > req.size {
                    return Err(GitXetError::internal("LFS download exceeds expected size"));
                }
                file.write_all(&chunk)?;
                progress.update_bytes_so_far(written)?;
            }
        }
        verify_download(path, &req.oid, req.size)
    }
}

// The LFS bridge URL includes the Xet hash even for objects without a Hub commit.
fn bridge_hash(href: &str) -> Option<String> {
    let url = Url::parse(href).ok()?;
    let parts: Vec<_> = url.path_segments()?.collect();
    let index = parts.iter().position(|p| p.starts_with("xet-bridge-"))?;
    let hash = *parts.get(index + 2)?;
    (hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit())).then(|| hash.to_owned())
}

fn verify_download(path: &Path, oid: &str, size: u64) -> Result<()> {
    let mut file = std::fs::File::open(path)?;
    if file.metadata()?.len() != size {
        return Err(GitXetError::internal("Incomplete LFS download"));
    }
    let mut digest = Sha256::new();
    let mut buffer = [0; 65536];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        digest.update(&buffer[..n]);
    }
    let hash: String = digest.finalize().iter().map(|byte| format!("{byte:02x}")).collect();
    if !hash.eq_ignore_ascii_case(oid) {
        return Err(GitXetError::internal("LFS download SHA-256 mismatch"));
    }
    Ok(())
}

fn http_error(error: reqwest::Error) -> GitXetError {
    GitXetError::internal(error.without_url())
}

#[cfg(test)]
#[path = "lfs_client_tests.rs"]
mod tests;
