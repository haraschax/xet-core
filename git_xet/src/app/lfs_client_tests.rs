use std::sync::Mutex;

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use tempfile::TempDir;

use super::*;
use crate::app::xet_agent::XetAgent;
use crate::lfs_agent_protocol::{InitRequestInner, TransferAgent};

const CONTENT: &[u8] = b"native git-xet download test";

fn request() -> TransferRequest {
    TransferRequest {
        oid: Sha256::digest(CONTENT).iter().map(|b| format!("{b:02x}")).collect(),
        size: CONTENT.len() as u64,
        path: None,
        action: GitBatchApiResponseAction::default(),
    }
}

fn repository() -> (TempDir, GitRepo) {
    let dir = TempDir::new().unwrap();
    git2::Repository::init(dir.path()).unwrap();
    let repo = GitRepo::open(dir.path()).unwrap();
    (dir, repo)
}

async fn serve(router: Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (endpoint, task)
}

#[test]
fn bridge_urls_and_endpoint_validation() {
    let hash = "ab".repeat(32);
    for host in ["cas-bridge.xethub.hf.co", "us.aws.cdn.hf.co"] {
        assert_eq!(bridge_hash(&format!("https://{host}/xet-bridge-us/repo/{hash}?signed=secret")), Some(hash.clone()));
    }
    assert_eq!(bridge_hash("https://example.com/objects/file"), None);
    assert_eq!(bridge_hash("https://example.com/xet-bridge-us/repo/not-a-hash"), None);
    assert!(remote_from_lfs_url("file:///tmp/repo/info/lfs").is_err());
    assert!(remote_from_lfs_url("https://huggingface.co/user/repo").is_err());
    let remote = remote_from_lfs_url("https://huggingface.co/datasets/user/repo.git/info/lfs/").unwrap();
    assert_eq!(remote.repo_info().unwrap().full_name, "user/repo");
}

#[tokio::test]
async fn basic_download_preserves_action_headers_and_verifies_content() {
    let app = Router::new().route(
        "/file",
        get(|headers: HeaderMap| async move {
            if headers.get("authorization").and_then(|v| v.to_str().ok()) == Some("Bearer action-token") {
                (StatusCode::OK, CONTENT)
            } else {
                (StatusCode::UNAUTHORIZED, b"".as_slice())
            }
        }),
    );
    let (endpoint, server) = serve(app).await;
    let (_dir, repo) = repository();
    let ctx = XetContext::default().unwrap();
    let mut client = LfsClient::new(&ctx, repo, format!("{endpoint}/user/repo.git").parse().unwrap(), None).unwrap();
    let mut req = request();
    req.action = GitBatchApiResponseAction {
        href: format!("{endpoint}/file?signature=secret"),
        header: HashMap::from([("authorization".into(), "Bearer action-token".into())]),
    };
    let file = tempfile::NamedTempFile::new().unwrap();
    for (oid, size, ok) in [
        (req.oid.clone(), req.size, true),
        ("00".repeat(32), req.size, false),
        (req.oid.clone(), req.size + 1, false),
        (req.oid.clone(), req.size - 1, false),
    ] {
        let req = TransferRequest {
            oid,
            size,
            ..req.clone()
        };
        let progress = ProgressUpdater::new(Arc::new(Mutex::new(Vec::new())), &req.oid);
        let result = client.download(&req, file.path(), progress).await;
        assert_eq!(result.is_ok(), ok, "{result:?}");
        if let Err(error) = result {
            assert!(!error.to_string().contains("secret"));
        }
    }
    server.abort();
}

#[tokio::test]
async fn batch_checks_objects_and_skips_existing_uploads() {
    let app = Router::new().route(
        "/user/repo.git/info/lfs/objects/batch",
        post(|Json(body): Json<serde_json::Value>| async move {
            Json(json!({ "transfer": "basic", "objects": body["objects"] }))
        }),
    );
    let (endpoint, server) = serve(app).await;
    let (_dir, repo) = repository();
    let mut client = LfsClient::new(
        &XetContext::default().unwrap(),
        repo,
        format!("{endpoint}/user/repo.git").parse().unwrap(),
        None,
    )
    .unwrap();
    assert!(client.batch(&request(), Operation::Upload).await.unwrap().is_none());
    let file = tempfile::NamedTempFile::new().unwrap();
    let progress = ProgressUpdater::new(Arc::new(Mutex::new(Vec::new())), &request().oid);
    assert!(
        client
            .download(&request(), file.path(), progress)
            .await
            .unwrap_err()
            .to_string()
            .contains("omitted")
    );
    server.abort();
}

#[cfg(feature = "simulation")]
#[tokio::test]
#[serial_test::serial(env_var_write_read)]
async fn private_batch_and_read_token_retry_with_credentials() {
    let _token = xet_runtime::utils::EnvVarGuard::set("HF_TOKEN", "test-credential");
    let anonymous = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let authorized = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let check = {
        let anonymous = anonymous.clone();
        let authorized = authorized.clone();
        move |headers: &HeaderMap| {
            let ok = headers.get("authorization").and_then(|v| v.to_str().ok()) == Some("Bearer test-credential");
            if ok { &authorized } else { &anonymous }.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            ok
        }
    };
    let batch_check = check.clone();
    let app = Router::new()
        .route(
            "/user/repo.git/info/lfs/objects/batch",
            post(move |headers: HeaderMap, Json(body): Json<serde_json::Value>| {
                let ok = batch_check(&headers);
                async move {
                    (
                        if ok { StatusCode::OK } else { StatusCode::UNAUTHORIZED },
                        Json(json!({ "objects": body["objects"] })),
                    )
                }
            }),
        )
        .route(
            "/api/models/user/repo/xet-read-token/main",
            get(move |headers: HeaderMap| {
                let ok = check(&headers);
                async move {
                    (
                        if ok { StatusCode::OK } else { StatusCode::UNAUTHORIZED },
                        Json(json!({
                            "casUrl": "http://127.0.0.1", "accessToken": "test-cas-token", "exp": 4102444800u64,
                        })),
                    )
                }
            }),
        );
    let (endpoint, server) = serve(app).await;
    for batch_first in [true, false] {
        let (_dir, repo) = repository();
        let mut client = LfsClient::new(
            &XetContext::default().unwrap(),
            repo,
            format!("{endpoint}/user/repo.git").parse().unwrap(),
            None,
        )
        .unwrap();
        if batch_first {
            assert!(client.batch(&request(), Operation::Upload).await.unwrap().is_none());
        }
        assert_eq!(client.read_token().await.unwrap().1.access_token, "test-cas-token");
    }
    assert_eq!(anonymous.load(std::sync::atomic::Ordering::SeqCst), 2);
    assert_eq!(authorized.load(std::sync::atomic::Ordering::SeqCst), 3);
    server.abort();
}

#[cfg(feature = "simulation")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial_test::serial(env_var_write_read)]
async fn native_upload_download_roundtrip_with_token_refresh() {
    use xet_client::cas_client::LocalTestServerBuilder;
    use xet_pkg::legacy::data_client;

    let ctx = XetContext::default().unwrap();
    let cas = LocalTestServerBuilder::new().start().await;
    let endpoint = cas.http_endpoint().to_string();
    let mut source = tempfile::NamedTempFile::new().unwrap();
    source.write_all(CONTENT).unwrap();
    let info = data_client::hash_files_async(&ctx, vec![source.path().to_string_lossy().into_owned()])
        .await
        .unwrap()
        .remove(0);
    let requests = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let app = Router::new().route("/api/models/user/repo/xet-read-token/main", get({
        let endpoint = endpoint.clone();
        let requests = requests.clone();
        move || { let endpoint = endpoint.clone(); let requests = requests.clone(); async move {
            // Force refresh after the initial metadata response.
            let n = requests.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Json(json!({ "casUrl": endpoint, "accessToken": "test", "exp": if n == 0 { 0 } else { 4102444800u64 } }))
        }}
    })).route("/user/repo.git/info/lfs/objects/batch", post(|State((hash, cas)): State<(String, String)>, Json(body): Json<serde_json::Value>| async move {
        let mut object = body["objects"][0].clone();
        let transfer = if body["operation"] == "upload" {
            object["actions"] = json!({ "upload": { "href": "http://127.0.0.1/unused-refresh", "header": {
                "X-Xet-Cas-Url": cas, "X-Xet-Access-Token": "test", "X-Xet-Token-Expiration": "4102444800",
            }}});
            "xet"
        } else {
            object["actions"] = json!({ "download": {"href": format!("https://unused.invalid/xet-bridge-us/repo/{hash}")} });
            "basic"
        };
        Json(json!({ "transfer": transfer, "objects": [object] }))
    })).with_state((info.hash, endpoint));
    let (hub, server) = serve(app).await;
    let mut agent = XetAgent::new(Some(format!("{hub}/user/repo.git/info/lfs")));
    let init = InitRequestInner {
        remote: "origin".into(),
        concurrent: false,
        concurrenttransfers: None,
    };
    agent.init_upload(&init).await.unwrap();
    let upload = TransferRequest {
        path: Some(source.path().to_owned()),
        ..request()
    };
    agent
        .upload_one(&upload, ProgressUpdater::new(Arc::new(Mutex::new(Vec::new())), &upload.oid))
        .await
        .unwrap();
    agent.init_download(&init).await.unwrap();
    let output = Arc::new(Mutex::new(Vec::new()));
    let path = agent
        .download_one(&request(), ProgressUpdater::new(output.clone(), &request().oid))
        .await
        .unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), CONTENT);
    assert!(requests.load(std::sync::atomic::Ordering::SeqCst) >= 2);
    assert!(!output.lock().unwrap().is_empty());
    drop(agent);
    assert!(!path.exists());
    server.abort();
}
