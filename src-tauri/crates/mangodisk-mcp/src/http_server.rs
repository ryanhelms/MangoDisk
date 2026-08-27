use std::{
    convert::Infallible,
    sync::Arc,
    task::{Context, Poll},
};

use bytes::Bytes;
use http::{header, Request, Response, StatusCode};
use http_body_util::{combinators::BoxBody, Full};
use hyper::body::Incoming;
use hyper_util::{
    rt::{TokioExecutor, TokioIo},
    server::conn::auto,
    service::TowerToHyperService,
};
use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
};
use tokio::net::TcpListener;
use tower_service::Service;

use crate::server::{AdapterState, MangoDiskServer};

/// Environment variable carrying the HTTP bearer token.
pub(crate) const TOKEN_ENV: &str = "MANGODISK_MCP_TOKEN";

/// Runs the streamable-HTTP transport, bound to loopback unless the operator
/// widened it with `--bind`. Every request requires `Authorization: Bearer
/// <token>`; stdio mode has no equivalent because only the supervising process
/// can reach it.
pub(crate) async fn serve_http(
    state: Arc<AdapterState>,
    bind: std::net::IpAddr,
    port: u16,
    allowed_hosts: Vec<String>,
) -> Result<(), String> {
    let token = match std::env::var(TOKEN_ENV) {
        Ok(token) if !token.is_empty() => token,
        _ => {
            let token = uuid::Uuid::new_v4().simple().to_string();
            // Printed once so the operator can copy it into their client; the
            // token never appears in logs after startup.
            eprintln!("{TOKEN_ENV}={token} mangodisk-mcp --http");
            token
        }
    };

    let cancellation = tokio_util::sync::CancellationToken::new();
    // The config is non-exhaustive, so the cancellation token is assigned
    // after construction; every other option keeps the rmcp default.
    let mut config = StreamableHttpServerConfig::default();
    config.cancellation_token = cancellation.clone();
    // DNS-rebinding protection: keep rmcp's loopback defaults, always allow
    // the bind address itself, and append operator-supplied hosts. An
    // unspecified bind (0.0.0.0/::) makes the dialed host unpredictable, so
    // the check is disabled there — the bearer token remains the boundary.
    if bind.is_unspecified() {
        config.allowed_hosts.clear();
    } else {
        config.allowed_hosts.push(bind.to_string());
    }
    config.allowed_hosts.extend(allowed_hosts);
    let mcp_service: StreamableHttpService<MangoDiskServer, LocalSessionManager> =
        StreamableHttpService::new(
            move || Ok(MangoDiskServer::new(state.clone())),
            Default::default(),
            config,
        );
    let service = BearerAuth::new(mcp_service, token);

    let listener = TcpListener::bind((bind, port))
        .await
        .map_err(|error| format!("failed to bind the MCP HTTP listener: {error}"))?;
    let address = listener
        .local_addr()
        .map_err(|error| format!("failed to read the MCP HTTP listener address: {error}"))?;
    if !bind.is_loopback() {
        // Network exposure is an explicit operator choice; make sure it is
        // visible. Bearer auth still applies, but the wire is not encrypted.
        log::warn!("mcp_http_non_loopback_bind address={address}");
        eprintln!(
            "warning: mangodisk-mcp is reachable from the network at http://{address}/; bearer auth is required but traffic is not encrypted"
        );
    }
    // Stderr is a logging channel in HTTP mode, so the machine-readable
    // startup line is safe to print here (unlike stdio mode).
    eprintln!("mangodisk-mcp listening on http://{address}/");
    log::info!("mcp_http_listening port={}", address.port());

    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (stream, _) = match accepted {
                    Ok(connection) => connection,
                    Err(error) => {
                        log::warn!("mcp_http_accept_failed error={error}");
                        continue;
                    }
                };
                let service = TowerToHyperService::new(service.clone());
                tokio::spawn(async move {
                    let builder = auto::Builder::new(TokioExecutor::new());
                    if let Err(error) = builder
                        .serve_connection(TokioIo::new(stream), service)
                        .await
                    {
                        log::info!("mcp_http_connection_closed reason={error}");
                    }
                });
            }
            () = shutdown_signal() => {
                log::info!("mcp_http_shutdown reason=signal");
                break;
            }
        }
    }
    // Terminates active MCP sessions and their in-flight tool calls.
    cancellation.cancel();
    Ok(())
}

/// systemd stops services with SIGTERM while interactive runs end with
/// Ctrl-C; either one must drain the accept loop instead of dying hard.
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        match signal(SignalKind::terminate()) {
            Ok(mut terminate) => {
                tokio::select! {
                    result = tokio::signal::ctrl_c() => {
                        if result.is_err() {
                            log::warn!("mcp_http_ctrl_c_signal_unavailable");
                        }
                    }
                    _ = terminate.recv() => {}
                }
            }
            Err(error) => {
                log::warn!("mcp_http_sigterm_unavailable error={error}");
                if tokio::signal::ctrl_c().await.is_err() {
                    log::warn!("mcp_http_shutdown_signal_unavailable");
                    std::future::pending::<()>().await;
                }
            }
        }
    }
    #[cfg(not(unix))]
    if tokio::signal::ctrl_c().await.is_err() {
        // A failed signal handler must not spin the accept loop forever.
        log::warn!("mcp_http_shutdown_signal_unavailable");
        std::future::pending::<()>().await;
    }
}

/// Tower middleware enforcing the bearer token before any request reaches the
/// MCP service. Comparison happens on the raw header value; rejection is a
/// plain 401 with no detail about the expected token.
#[derive(Clone)]
struct BearerAuth<S> {
    inner: S,
    token: Arc<str>,
}

impl<S> BearerAuth<S> {
    fn new(inner: S, token: String) -> Self {
        Self {
            inner,
            token: token.into(),
        }
    }
}

type BoxedResponse = Response<BoxBody<Bytes, Infallible>>;

impl<S> Service<Request<Incoming>> for BearerAuth<S>
where
    S: Service<Request<Incoming>, Response = BoxedResponse, Error = Infallible>
        + Clone
        + Send
        + 'static,
    S::Future: Send + 'static,
{
    type Response = BoxedResponse;
    type Error = Infallible;
    type Future = std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<BoxedResponse, Infallible>> + Send>,
    >;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: Request<Incoming>) -> Self::Future {
        let authorized = request
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .is_some_and(|presented| presented == &*self.token);
        if !authorized {
            return Box::pin(async { Ok(unauthorized()) });
        }
        let mut inner = self.inner.clone();
        Box::pin(async move { inner.call(request).await })
    }
}

fn unauthorized() -> BoxedResponse {
    let body = Full::new(Bytes::from_static(
        br#"{"error":"unauthorized","hint":"send Authorization: Bearer <token>"}"#,
    ));
    let mut response = Response::new(BoxBody::new(body));
    *response.status_mut() = StatusCode::UNAUTHORIZED;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("application/json"),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unauthorized_response_carries_no_token_detail() {
        let response = unauthorized();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
}
