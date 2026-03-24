//! Binary entrypoint for the cf-speedtest-v2 project.

use core::iter;
use core::net::{IpAddr, Ipv4Addr, SocketAddr};
use core::num::NonZeroU64;
use std::collections::HashMap;
use std::env;
use std::sync::LazyLock;

use axum::body::Body;
use axum::extract::Request;
use axum::http::header::{ACCEPT_ENCODING, CACHE_CONTROL, CONTENT_ENCODING, CONTENT_TYPE};
use axum::http::response::Parts;
use axum::http::{HeaderName, HeaderValue, StatusCode};
use axum::response::Response;
use axum::routing::get;
use bytes::Bytes;
use cf_speedtest_core::{DEFAULT_BYTES, MAX_BYTES};
use fastrace_axum::FastraceLayer;
use memchr::{Memchr, memchr};
use tokio::net::TcpListener;

// #[global_allocator]
// static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

const LISTENING_ADDR_ENV: &str = "CF_SPEEDTEST_LISTEN";
const LISTENING_ADDR_DEFAULT: SocketAddr =
    SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 1585);

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // fastrace::set_reporter(
    //     fastrace::collector::ConsoleReporter,
    //     fastrace::collector::Config::default(),
    // );

    {
        use logforth::append::{FastraceEvent, Stdout};
        use logforth::filter::env_filter::EnvFilterBuilder;
        use logforth::layout::TextLayout;

        logforth::bridge::log::setup();
        logforth::core::builder()
            .dispatch(|d| {
                d.filter(EnvFilterBuilder::from_env_or("CF_SPEEDTEST_LOG", "debug").build())
                    .append(Stdout::default().with_layout(TextLayout::default()))
            })
            .dispatch(|d| d.append(FastraceEvent::default()))
            .apply();
    }

    let listen = env::var(LISTENING_ADDR_ENV).map_or_else(
        |_| Ok(LISTENING_ADDR_DEFAULT),
        |val| {
            val.parse()
                .inspect_err(|_| log::error!("invalid address: {val}"))
        },
    )?;

    log::info!("Listening on {listen}... Press Ctrl+C to stop.");

    let _ = axum::serve(
        TcpListener::bind(listen).await?,
        axum::Router::new()
            .route(
                "/speedtest",
                get(handler)
                    .head(async || status(StatusCode::OK, None))
                    .fallback(async || status(StatusCode::METHOD_NOT_ALLOWED, None)),
            )
            .route(
                "/speedtest/",
                get(handler)
                    .head(async || status(StatusCode::OK, None))
                    .fallback(async || status(StatusCode::METHOD_NOT_ALLOWED, None)),
            )
            .fallback(async || status(StatusCode::NOT_FOUND, None))
            .layer(FastraceLayer::default()),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await;

    fastrace::flush();

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
            log::info!("Received SIGHUP");
        }
    }
}

#[inline]
fn status(status: StatusCode, body: Option<&'static str>) -> Response {
    let mut response = Response::new(body.map(Into::into).unwrap_or_else(Body::empty));

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

#[inline]
fn zeros(body: impl Into<Body>) -> Response {
    let mut response = Response::new(body.into());

    *response.headers_mut() = [
        (
            CACHE_CONTROL,
            HeaderValue::from_static("public, s-maxage=31536000, max-age=0"),
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

static CACHE: LazyLock<Cache> = LazyLock::new(Cache::default);

struct Cache {
    cached: HashMap<NonZeroU64, (Parts, Bytes), foldhash::fast::RandomState>,
}

impl Default for Cache {
    fn default() -> Self {
        macro_rules! nonzero {
            ($val:expr) => {{
                debug_assert!($val > 0, "value must be greater than zero");

                #[allow(unsafe_code, reason = "XXX")]
                unsafe {
                    NonZeroU64::new_unchecked($val)
                }
            }};
        }

        Self {
            cached: [
                nonzero!(50 * 1024 * 1024),
                nonzero!(100 * 1024 * 1024),
                nonzero!(200 * 1024 * 1024),
                nonzero!(300 * 1024 * 1024),
                nonzero!(500 * 1024 * 1024),
                nonzero!(1024 * 1024 * 1024),
                nonzero!(10 * 1024 * 1024 * 1024),
            ]
            .into_iter()
            .map(|b| {
                let bytes = Bytes::from(cf_speedtest_core::zeros(b));

                let (parts, _) = zeros(bytes.clone()).into_parts();

                (b, (parts, bytes))
            })
            .collect(),
        }
    }
}

#[fastrace::trace(enter_on_poll = true)]
async fn handler(request: Request) -> Response {
    // Reject requests with `zstd` content encoding, as Cloudflare will deliver our
    // compressed response directly to the client without decompressing it.
    if request
        .headers()
        .get(ACCEPT_ENCODING)
        .is_some_and(|encoding| memchr::memmem::find(encoding.as_bytes(), b"zstd").is_some())
    {
        return status(
            StatusCode::BAD_REQUEST,
            Some("FATAL: the client should not accept `zstd` encoding."),
        );
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

    log::debug!("The client requests {bytes} bytes.");

    CACHE
        .cached
        .get(&bytes)
        .map(|(parts, bytes)| Response::from_parts(parts.clone(), Body::from(bytes.clone())))
        .unwrap_or_else(|| zeros(Bytes::from(cf_speedtest_core::zeros(bytes))))
}
