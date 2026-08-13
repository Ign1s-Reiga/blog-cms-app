//! The bearer-token gate, exercised over a real HTTP server.
//!
//! This is the check that cannot be made by reading the code: if the auth layer
//! were attached so that it did not actually wrap the nested MCP service, every
//! one of these requests would still be answered — the endpoint would be open to
//! anything on the machine that guessed the port, and nothing would fail to
//! compile. So the server is started for real and spoken to over TCP.
//!
//! A stub handler stands in for `BlogMcp`, which needs a running Tauri app. What
//! is under test is the transport and the gate in front of it, not the tools.

use std::sync::Arc;

use rmcp::model::{ServerCapabilities, ServerInfo};
use rmcp::ServerHandler;
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};

const TOKEN: &str = "correct-horse-battery-staple";

/// The MCP `initialize` handshake — the first thing any client sends.
const INITIALIZE: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}"#;

#[derive(Clone)]
struct Stub;

impl ServerHandler for Stub {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
    }
}

/// Start the endpoint on an ephemeral port and return its base URL.
async fn serve() -> String {
    let service = StreamableHttpService::new(
        || Ok(Stub),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default(),
    );
    let router = app_lib::mcp::build_router(service, Arc::new(TOKEN.to_string()));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let port = listener.local_addr().expect("local addr").port();

    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });

    format!("http://127.0.0.1:{port}{}", app_lib::mcp::ENDPOINT_PATH)
}

/// Send `initialize`, optionally bearing a token.
async fn initialize(url: &str, bearer: Option<&str>) -> reqwest::StatusCode {
    let mut request = reqwest::Client::new()
        .post(url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .body(INITIALIZE);

    if let Some(token) = bearer {
        request = request.header("Authorization", format!("Bearer {token}"));
    }

    request.send().await.expect("request sent").status()
}

#[tokio::test]
async fn the_right_token_gets_through() {
    let url = serve().await;
    let status = initialize(&url, Some(TOKEN)).await;
    assert!(
        status.is_success(),
        "a correctly authorised initialize should be served, got {status}"
    );
}

#[tokio::test]
async fn every_wrong_credential_is_refused() {
    let url = serve().await;

    for (label, bearer) in [
        ("no Authorization header at all", None),
        ("an empty bearer", Some("")),
        ("a wrong token", Some("guessed")),
        // Length is the one thing the comparison leaks, so a prefix of the real
        // token is worth pinning explicitly.
        ("a prefix of the real token", Some(&TOKEN[..8])),
        ("the token with trailing junk", Some("correct-horse-battery-staple!")),
    ] {
        let status = initialize(&url, bearer).await;
        assert_eq!(
            status,
            reqwest::StatusCode::UNAUTHORIZED,
            "{label} must be rejected, got {status}"
        );
    }
}

/// The gate has to cover the whole mount, not just the exact path — a client
/// poking at a sub-path should get the same 401 rather than reaching the
/// service.
#[tokio::test]
async fn unauthorised_requests_are_refused_on_every_path() {
    let base = serve().await;

    for path in ["", "/", "/anything"] {
        let status = initialize(&format!("{base}{path}"), None).await;
        assert_eq!(
            status,
            reqwest::StatusCode::UNAUTHORIZED,
            "unauthorised request to `{path}` must be rejected, got {status}"
        );
    }
}
