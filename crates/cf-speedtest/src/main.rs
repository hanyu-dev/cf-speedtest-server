//! Binary entrypoint for the cf-speedtest-v2 project.

use std::env;
use std::net::SocketAddr;
use std::sync::LazyLock;

use axum::extract::Request;
use axum::http::header::{CACHE_CONTROL, CONTENT_ENCODING, CONTENT_TYPE};
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use dashmap::DashMap;
use macro_toolset::init_tracing_simple;
use tokio::net::TcpListener;

// // Mimalloc
// #[global_allocator]
// static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    init_tracing_simple!();

    tracing::info!("Starting server, version: {}", cf_speedtest_core::VERSION);

    let listen: SocketAddr = env::var("CF_SPEEDTEST_LISTEN")
        .unwrap_or_else(|_| {
            tracing::warn!("CF_SPEEDTEST_LISTEN not set, using default: 127.0.0.1:8000");
            String::new()
        })
        .parse()
        .unwrap_or(SocketAddr::from(([0, 0, 0, 0], 8000)));

    let tcp_listener = TcpListener::bind(listen).await?;

    // Main Server
    let _ = axum::serve(tcp_listener, axum::Router::new().fallback(handler))
        .with_graceful_shutdown(shutdown_signal())
        .await;

    Ok(())
}

/// axum graceful shutdown signal
async fn shutdown_signal() {
    #[cfg(unix)]
    let hangup = async {
        use tokio::signal::unix::{signal, SignalKind};
        signal(SignalKind::hangup()).unwrap().recv().await;
    };

    #[cfg(not(unix))]
    let hangup = std::future::pending::<()>();

    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = hangup => {
            tracing::info!("Received SIGHUP");
        }
    }
}

type CacheMap = DashMap<String, (&'static [u8], Box<HeaderMap>), foldhash::fast::RandomState>;

static CACHE: LazyLock<CacheMap> =
    LazyLock::new(|| DashMap::with_capacity_and_hasher(16, foldhash::fast::RandomState::default()));

#[inline(always)]
#[tracing::instrument]
async fn handler(request: Request) -> Response {
    tracing::debug!("Accepted request.");

    let path = request.uri().path();

    let (body, headers) = match CACHE.get(path) {
        Some(value) => (value.0, *value.1.clone()),
        _ => {
            tracing::debug!("Cache miss, create new one.");

            let body = match cf_speedtest_core::body(path) {
                Some(body) => body,
                None => {
                    tracing::warn!("Invalid path: {}", path);

                    return StatusCode::NOT_FOUND.into_response();
                }
            };

            // Make compiler happy here: `: HeaderMap`
            let headers: HeaderMap = [
                (
                    CACHE_CONTROL,
                    HeaderValue::from_static("public, max-age=31536000"),
                ),
                (CONTENT_TYPE, HeaderValue::from_static("text/plain")),
                (CONTENT_ENCODING, HeaderValue::from_static("br")),
                (
                    HeaderName::from_static("x-server"),
                    HeaderValue::from_static(cf_speedtest_core::VERSION),
                ),
            ]
            .into_iter()
            .collect();

            {
                let path = path.to_string();
                let headers = Box::new(headers.clone());
                tokio::spawn(async move {
                    CACHE.insert(path, (body, headers));
                });
            }

            (body, headers)
        }
    };

    let mut response = Response::new(body.into());

    *response.headers_mut() = headers;

    response
}
