//! WASM binary for Cloudflare Workers deployment.

use std::io;
use std::num::NonZeroU64;

use fluent_uri::Uri;
use tespeed::DecompressionBomb;
use worker::js_sys::Uint8Array;
use worker::web_sys::{Headers, Request, Response, ResponseInit};
use worker::worker_sys::ext::ResponseInitExt;
use worker::{Context, Env, Result, event};

/// Route prefix for the speedtest worker.
const WORKER_ROUTE_PREFIX: &str = "/speedtest";

#[event(fetch)]
async fn fetch(req: Request, _env: Env, _ctx: Context) -> Result<Response> {
    console_error_panic_hook::set_once();

    // Filter HTTP method.
    match req.method() {
        method if method.eq_ignore_ascii_case("GET") => {
            // OK, Do nothing.
        }
        method if method.eq_ignore_ascii_case("HEAD") => {
            return status(200, None);
        }
        _ => {
            return status(405, None);
        }
    }

    let url = req.url();

    let uri = Uri::try_from(url.as_str()).map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("invalid url `{url}`: {e}"),
        )
    })?;

    let bytes = uri
        .query()
        .iter()
        .flat_map(|query| query.as_str().split('&'))
        .find_map(|pair| {
            let mut split = pair.split('=');

            let Some(k) = split.next() else {
                return None;
            };

            if !k.eq_ignore_ascii_case("bytes") {
                return None;
            }

            split.next().and_then(|v| v.parse::<NonZeroU64>().ok())
        })
        .or_else(|| {
            let bytes = uri
                .path()
                .as_str()
                .trim_start_matches(WORKER_ROUTE_PREFIX)
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
        .map_or_else(DecompressionBomb::default, |size| {
            DecompressionBomb::new(size).unwrap_or_default()
        });

    let body = bytes.zstd();

    #[allow(
        unsafe_code,
        reason = "Will never resize the allocated buffer after that the function returns."
    )]
    let body = unsafe {
        let (ptr, length, _capacity) = body.into_raw_parts();
        Uint8Array::view_mut_raw(ptr, length)
    };

    RESPONSE_INIT.with(|init| {
        Response::new_with_opt_buffer_source_and_init(Some(&body), init).map_err(Into::into)
    })
}

thread_local! {
    static RESPONSE_INIT: ResponseInit =  {
        let headers = Headers::new().expect("Failed to create headers");

        headers
            .append("x-server", env!("CARGO_PKG_VERSION"))
            .expect("Failed to append `x-server` header");
        headers
            .append("cache-control", "public, s-maxage=31536000, max-age=0")
            .expect("Failed to append `cache-control` header");
        headers
            .append("content-type", "application/octet-stream")
            .expect("Failed to append `content-type` header");
        headers
            .append("content-encoding", "zstd")
            .expect("Failed to append `content-encoding` header");

        let mut init = ResponseInit::new();

        init.set_status(200);
        init.set_headers(&headers);
        init.encode_body("manual")
            .expect("Failed to configure `manual` body encoding");

        init
    }
}

fn status(status: u16, message: Option<&str>) -> Result<Response> {
    let headers = Headers::new().expect("Failed to create headers");

    headers.append("x-server", env!("CARGO_PKG_VERSION"))?;
    headers
        .append("cache-control", "public, s-maxage=31536000, max-age=0")
        .expect("Failed to append `cache-control` header");

    let init = ResponseInit::new();
    init.set_status(status);
    init.set_headers(&headers);

    Response::new_with_opt_str_and_init(message, &init).map_err(Into::into)
}
