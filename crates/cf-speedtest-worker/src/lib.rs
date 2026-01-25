//! WASM binary for Cloudflare Workers deployment.

#![feature(option_reference_flattening)]

use std::collections::HashMap;
use std::io;
use std::num::NonZeroU64;

use worker::js_sys::{Reflect, Uint8Array};
use worker::wasm_bindgen::JsValue;
use worker::web_sys::{Headers, Request, Response, ResponseInit};
use worker::worker_sys::ext::{RequestExt, ResponseInitExt};
use worker::worker_sys::types::IncomingRequestCfProperties;
use worker::{Context, Env, Result, event};

/// Route prefix for the speedtest worker.
const WORKER_ROUTE_PREFIX: &str = "/speedtest";

/// If missing bytes indication, or invalid, assume 200 MiB.
const DEFAULT_BYTES: NonZeroU64 = NonZeroU64::new(200 * 1024 * 1024).unwrap();

/// Maximum bytes allowed to be requested.
///
/// Currently set to 10 GiB.
const MAX_BYTES: NonZeroU64 = NonZeroU64::new(10 * 1024 * 1024 * 1024).unwrap();

#[event(fetch)]
async fn fetch(req: Request, _env: Env, _ctx: Context) -> Result<Response> {
    console_error_panic_hook::set_once();

    // Filter HTTP method.
    match req.method() {
        method if method.eq_ignore_ascii_case("GET") => {
            // OK, Do nothing.
        }
        method if method.eq_ignore_ascii_case("HEAD") => {
            return build_general_response(None, 200);
        }
        _ => {
            return build_general_response(None, 405);
        }
    }

    let cf = req.cf();

    // Filter Accept-Encoding header.
    if let Some(ret) = || -> Option<Result<Response, worker::Error>> {
        let enc;

        macro_rules! ret {
            ($($ret:tt)+) => {{
                match $($ret)+ {
                    Ok(ret) => ret,
                    Err(e) => return Some(Err(e.into())),
                }
            }};
        }

        if let Some(client_accept_encoding) = ret!(get_client_accept_encoding(cf.as_ref())) {
            enc = client_accept_encoding
        } else if let Some(accept_encoding) = ret!(req.headers().get("accept-encoding")) {
            enc = accept_encoding
        } else {
            return None;
        }

        enc.rsplit(',')
            .map(|enc| enc.trim())
            .find(|enc| enc.eq_ignore_ascii_case(cf_speedtest_core::CONTENT_ENCODING))
            .map(|_| {
                build_general_response(
                    Some(constcat::concat!(
                        "The client should not accept encoding `",
                        cf_speedtest_core::CONTENT_ENCODING,
                        "`"
                    )),
                    406,
                )
            })
    }() {
        return ret;
    }

    let uri = req.url();
    let uri = fluent_uri::Uri::try_from(uri.as_str()).map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("invalid uri `{uri}`: {e}"),
        )
    })?;

    let query = uri
        .query()
        .iter()
        .flat_map(|query| query.as_str().split('&'))
        .flat_map(|pair| {
            let mut split = pair.split('=');

            let k = split.next()?;
            let v = split.next();

            Some((k, v))
        })
        .collect::<HashMap<_, _, foldhash::fast::RandomState>>();

    let bytes = query
        .get("bytes")
        .flatten_ref()
        .and_then(|v| v.parse().ok())
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
        .unwrap_or(DEFAULT_BYTES)
        .min(MAX_BYTES);

    let body = cf_speedtest_core::zeros(bytes);

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
            .append("x-server", cf_speedtest_core::VERSION)
            .expect("Failed to append `x-server` header");
        headers
            .append("content-type", "application/octet-stream")
            .expect("Failed to append `content-type` header");
        headers
            .append("content-encoding", cf_speedtest_core::CONTENT_ENCODING)
            .expect("Failed to append `content-encoding` header");

        let mut init = ResponseInit::new();
        init.set_status(200);
        init.set_headers(&headers);
        init.encode_body("manual")
            .expect("Failed to configure `manual` body encoding");

        init
    }
}

fn build_general_response(message: Option<&str>, status: u16) -> Result<Response> {
    let headers = Headers::new().expect("Failed to create headers");

    headers.append("x-server", cf_speedtest_core::VERSION)?;

    let init = ResponseInit::new();
    init.set_status(status);
    init.set_headers(&headers);

    Response::new_with_opt_str_and_init(message, &init).map_err(Into::into)
}

fn get_client_accept_encoding(
    obj: Option<&IncomingRequestCfProperties>,
) -> Result<Option<String>, JsValue> {
    let obj = match obj {
        Some(obj) => obj,
        None => return Ok(None),
    };

    let value = Reflect::get(obj, &JsValue::from_str("clientAcceptEncoding"))?;

    if value.is_undefined() || value.is_null() {
        Ok(None)
    } else {
        value
            .as_string()
            .ok_or_else(|| {
                JsValue::from_str("Failed to convert `clientAcceptEncoding` property to String")
            })
            .map(Some)
    }
}
