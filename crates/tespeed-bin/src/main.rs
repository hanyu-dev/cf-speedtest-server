//! Minimum `tespeed` server.

use std::collections::HashMap;
use std::num::NonZeroU64;
use std::sync::LazyLock;
use std::time::Duration;
use std::{io, iter};

use anyhow::Result;
use axum::body::{Body, Bytes};
use axum::extract::Request;
use axum::http::header::{ACCEPT_ENCODING, CACHE_CONTROL, CONTENT_ENCODING, CONTENT_TYPE};
use axum::http::response::Parts;
use axum::http::{HeaderName, HeaderValue, StatusCode};
use axum::response::Response;
use axum::routing::get;
use clap::Parser as _;
use fastrace_axum::FastraceLayer;
use memchr::memmem::find;
use memchr::{Memchr, memchr};
use sockext::net::{SockAddr, TcpStream};
use tespeed::DecompressionBomb;
use tokio::time::sleep;

#[cfg(not(target_env = "msvc"))]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

#[tokio::main]
async fn main() -> io::Result<()> {
    let args = Args::parse();

    {
        use logforth::append::Stdout;
        use logforth::bridge::log::LogBridge;
        use logforth::layout::TextLayout;
        use logforth::record::{Level, LevelFilter};

        let logger = logforth::core::builder()
            .dispatch(|d| {
                d.filter(LevelFilter::MoreSevereEqual(Level::Debug))
                    .append(Stdout::default().with_layout(TextLayout::default()))
            })
            .build();

        log::set_max_level(log::LevelFilter::Trace);
        log::set_boxed_logger(Box::new(LogBridge::new(logger))).unwrap();
    }

    log::info!(
        listen:% = &args.listen;
        "server started"
    );

    let ret = axum::serve(
        TcpListener::bind(&args.listen)?,
        axum::Router::new()
            .route(
                "/speedtest",
                get(handler)
                    .head(handler_ok)
                    .fallback(handler_method_not_allowed),
            )
            .route(
                "/speedtest/",
                get(handler)
                    .head(handler_ok)
                    .fallback(handler_method_not_allowed),
            )
            .fallback(handler_not_found)
            .layer(FastraceLayer::default()),
    )
    .with_graceful_shutdown(signal())
    .await;

    fastrace::flush();

    ret
}

#[fastrace::trace]
async fn handler(request: Request) -> Result<Response, StatusCode> {
    log::debug!(
        trace:?= request.headers().get("traceparent"),
        method:% = request.method(),
        uri:% = &request.uri();
        "received request"
    );

    // Reject requests with `zstd` content encoding, as Cloudflare will deliver our
    // compressed response directly to the client without decompressing it.
    if request
        .headers()
        .get(ACCEPT_ENCODING)
        .is_some_and(|encoding| find(encoding.as_bytes(), b"zstd").is_some())
    {
        return Err(StatusCode::BAD_REQUEST);
    }

    let bomber = request
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
        .map_or_else(
            || Ok(DecompressionBomb::default()),
            |size| DecompressionBomb::new(size).map_err(|_| StatusCode::BAD_REQUEST),
        )?;

    log::debug!(
        method:% = request.method(),
        uri:% = &request.uri();
        "bomber size: {} bytes", bomber.size()
    );

    Ok(Response::from_parts(
        RESPONSE_FIXED_PARTS.clone(),
        GLOBAL_CACHED_ZSTD_COMPRESSED_PAYLOAD
            .get(&bomber.size())
            .map_or_else(|| bomber.zstd().into(), |bytes| bytes.clone().into()),
    ))
}

static GLOBAL_CACHED_ZSTD_COMPRESSED_PAYLOAD: LazyLock<
    HashMap<NonZeroU64, Bytes, foldhash::fast::RandomState>,
> = LazyLock::new(|| {
    macro_rules! nonzero {
        ($val:expr) => {{
            debug_assert!($val > 0, "value must be greater than zero");

            #[allow(unsafe_code, reason = "XXX")]
            unsafe {
                NonZeroU64::new_unchecked($val)
            }
        }};
    }

    [
        nonzero!(50u64 * 1024 * 1024),
        nonzero!(100u64 * 1024 * 1024),
        nonzero!(200u64 * 1024 * 1024),
        nonzero!(300u64 * 1024 * 1024),
        nonzero!(500u64 * 1024 * 1024),
        nonzero!(1024u64 * 1024 * 1024),
        nonzero!(10u64 * 1024 * 1024 * 1024),
    ]
    .into_iter()
    .map(|b| {
        (
            b,
            Bytes::from(
                DecompressionBomb::new(b)
                    .expect("infallible: the length is guaranteed to be less than 10 GiB")
                    .zstd(),
            ),
        )
    })
    .collect()
});

async fn handler_ok() -> Response {
    status(StatusCode::OK, None)
}

async fn handler_method_not_allowed() -> Response {
    status(StatusCode::METHOD_NOT_ALLOWED, None)
}

async fn handler_not_found() -> Response {
    status(StatusCode::NOT_FOUND, None)
}

#[inline]
fn status(status: StatusCode, body: Option<&'static str>) -> Response {
    // Fucking axum's API design!!!
    Response::builder()
        .status(status)
        .header(CONTENT_TYPE, HeaderValue::from_static("text/plain"))
        .header(
            HeaderName::from_static("x-server"),
            HeaderValue::from_static(env!("CARGO_PKG_VERSION")),
        )
        .body(body.map(Into::into).unwrap_or_else(Body::empty))
        .expect("infallible")
}

static RESPONSE_FIXED_PARTS: LazyLock<Parts> = LazyLock::new(|| {
    let (parts, _) = Response::builder()
        .header(
            CACHE_CONTROL,
            HeaderValue::from_static("public, s-maxage=31536000, max-age=0"),
        )
        .header(
            CONTENT_TYPE,
            HeaderValue::from_static("application/octet-stream"),
        )
        .header(CONTENT_ENCODING, HeaderValue::from_static("zstd"))
        .header(
            HeaderName::from_static("x-server"),
            HeaderValue::from_static(env!("CARGO_PKG_VERSION")),
        )
        .body(Body::empty())
        .unwrap()
        .into_parts();

    parts
});

// === Glue codes ===

#[derive(Debug)]
#[derive(clap::Parser)]
struct Args {
    #[clap(short, long, default_value = "127.0.0.1:1585")]
    /// The address to listen on.
    listen: SockAddr,
}

wrapper_lite::wrapper!(
    #[wrapper(DerefMut)]
    struct TcpListener(sockext::net::TcpListener);
);

impl TcpListener {
    fn bind(addr: &SockAddr) -> io::Result<Self> {
        sockext::net::TcpListener::bind(addr).map(Self::from_inner)
    }
}

impl axum::serve::Listener for TcpListener {
    type Addr = SockAddr;
    type Io = TcpStream;

    fn accept(&mut self) -> impl Future<Output = (Self::Io, Self::Addr)> + Send {
        async {
            loop {
                match self.inner.accept().await {
                    Ok(ret) => break ret,
                    Err(e) => {
                        log::error!("failed to accept connection: {e}");

                        sleep(Duration::from_secs(1)).await
                    }
                }
            }
        }
    }

    fn local_addr(&self) -> io::Result<Self::Addr> {
        self.inner.local_addr()
    }
}

async fn signal() {
    let hangup = cfg_select! {
        unix => {
            async {
                use tokio::signal::unix::{SignalKind, signal};

                signal(SignalKind::hangup())
                    .expect("fatal: failed to install SIGHUP handler")
                    .recv()
                    .await;
            }
        }
        _ => std::future::pending::<()>(),
    };

    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = hangup => {
            log::info!("Received SIGHUP");
        }
    }
}
