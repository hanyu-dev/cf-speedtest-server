//! Binary entrypoint for the cf-speedtest-v2 project.

use core::iter;
use core::net::{IpAddr, Ipv4Addr, SocketAddr};
use core::num::NonZeroU64;
use std::env;

use axum::body::Body;
use axum::extract::Request;
use axum::http::header::{CACHE_CONTROL, CONTENT_ENCODING, CONTENT_TYPE};
use axum::http::response::Parts;
use axum::http::{HeaderName, HeaderValue, StatusCode};
use axum::response::Response;
use axum::routing::get;
use bytes::Bytes;
use cf_speedtest_core::{DEFAULT_BYTES, MAX_BYTES};
use dashmap::DashMap;
use macro_toolset::init_tracing_simple;
use memchr::{Memchr, memchr};
use tokio::net::TcpListener;

// // Mimalloc
// #[global_allocator]
// static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

const LISTEN_ADDR_ENV: &str = "CF_SPEEDTEST_LISTEN";
const LISTEN_ADDR_DEFAULT: SocketAddr =
    SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), 8000);

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    init_tracing_simple!();

    tracing::info!("Starting server, version: {}", cf_speedtest_core::VERSION);

    let listen: SocketAddr = env::var(LISTEN_ADDR_ENV)
        .unwrap_or_default()
        .parse()
        .unwrap_or(LISTEN_ADDR_DEFAULT);

    let _ = axum::serve(
        TcpListener::bind(listen).await?,
        axum::Router::new()
            .route(
                "/speedtest",
                get(handler)
                    .head(async || status(StatusCode::OK))
                    .fallback(async || status(StatusCode::METHOD_NOT_ALLOWED)),
            )
            .route(
                "/speedtest/",
                get(handler)
                    .head(async || status(StatusCode::OK))
                    .fallback(async || status(StatusCode::METHOD_NOT_ALLOWED)),
            )
            .fallback(async || status(StatusCode::NOT_FOUND)),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await;

    Ok(())
}

/// axum graceful shutdown signal
async fn shutdown_signal() {
    #[cfg(unix)]
    let hangup = async {
        use tokio::signal::unix::{SignalKind, signal};
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

thread_local! {
    static CACHE: DashMap<NonZeroU64, (Parts, Bytes), foldhash::fast::RandomState> = DashMap::default();
}

#[inline]
fn status(status: StatusCode) -> Response {
    let mut response = Response::new(Body::empty());

    *response.status_mut() = status;

    *response.headers_mut() = [
        (CONTENT_TYPE, HeaderValue::from_static("text/plain")),
        (
            HeaderName::from_static("x-server"),
            HeaderValue::from_static(cf_speedtest_core::VERSION),
        ),
    ]
    .into_iter()
    .collect();

    response
}

fn zeros(body: impl Into<Body>) -> Response {
    let mut response = Response::new(body.into());

    *response.headers_mut() = [
        (
            CACHE_CONTROL,
            HeaderValue::from_static("public, max-age=31536000"),
        ),
        (
            CONTENT_TYPE,
            HeaderValue::from_static("application/octet-stream"),
        ),
        (
            CONTENT_ENCODING,
            HeaderValue::from_static(cf_speedtest_core::CONTENT_ENCODING),
        ),
        (
            HeaderName::from_static("x-server"),
            HeaderValue::from_static(cf_speedtest_core::VERSION),
        ),
    ]
    .into_iter()
    .collect();

    response
}

#[inline(always)]
async fn handler(request: Request) -> Response {
    tracing::debug!("Accepted request.");

    // Reject requests with `zstd` content encoding, as Cloudflare will have our
    // compressed response returned immediately to the client without decompressing
    // it.
    if request
        .headers()
        .get(CONTENT_ENCODING)
        .is_some_and(|encoding| memchr::memmem::find(encoding.as_bytes(), b"zstd").is_some())
    {
        return status(StatusCode::BAD_REQUEST);
    }

    let bytes = request
        .uri()
        .query()
        .and_then(|query| {
            let iter = Memchr::new(b'&', query.as_bytes());

            iter::once(0)
                .chain(iter.clone().map(|idx| idx + 1))
                .zip(iter.chain(iter::once(query.len())))
                .map(|(start, end)| {
                    let query = &query[start..end];

                    match memchr(b'=', query.as_bytes()) {
                        Some(idx) => (&query[..idx], &query[idx + 1..]),
                        None => (query, ""),
                    }
                })
                .find_map(|(k, v)| {
                    if k.eq_ignore_ascii_case("bytes") {
                        v.parse::<NonZeroU64>().ok()
                    } else {
                        None
                    }
                })
        })
        .or_else(|| {
            let bytes = request
                .uri()
                .path()
                .trim_start_matches("/")
                .trim_end_matches(".test");

            let offset = bytes.rfind(|c: char| c.is_numeric())? + 1;
            let base: u64 = (&bytes[..offset]).parse().ok()?;
            let unit: u64 = match &bytes[offset..] {
                unit if unit.is_empty() || unit.eq_ignore_ascii_case("B") => 1,
                unit if unit.eq_ignore_ascii_case("KB") => 1000,
                unit if unit.eq_ignore_ascii_case("KiB") => 1024,
                unit if unit.eq_ignore_ascii_case("MB") => 1000 * 1000,
                unit if unit.eq_ignore_ascii_case("MiB") => 1024 * 1024,
                unit if unit.eq_ignore_ascii_case("GB") => 1000 * 1000 * 1000,
                unit if unit.eq_ignore_ascii_case("GiB") => 1024 * 1024 * 1024,
                _ => return None,
            };

            NonZeroU64::new(base * unit)
        })
        .unwrap_or(DEFAULT_BYTES)
        .min(MAX_BYTES);

    tracing::debug!("Requesting {bytes} bytes.");

    let response = match CACHE.try_with(|cache| {
        cache.get(&bytes).map(|cache| {
            let (parts, body) = cache.value();
            Response::from_parts(parts.clone(), Body::from(body.clone()))
        })
    }) {
        Ok(Some(response)) => response,
        _ => {
            let body = Bytes::from(cf_speedtest_core::zeros(bytes));

            let (parts, opaque) = zeros(body.clone()).into_parts();

            CACHE
                .try_with(|cache| {
                    cache.insert(bytes, (parts.clone(), body));
                })
                .ok();

            Response::from_parts(parts, opaque)
        }
    };

    response
}
