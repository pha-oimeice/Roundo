//! Definition-local custom protocol resource loading.

use super::*;
use std::borrow::Cow;

#[cfg(target_os = "windows")]
pub(super) fn protocol_response(
    registry: &UiRegistry,
    source_resource: Option<&str>,
    uri: &wry::http::Uri,
) -> wry::http::Response<Cow<'static, [u8]>> {
    let authority = uri
        .authority()
        .map(|value| value.as_str())
        .unwrap_or_default();
    log::debug!("Web UI custom-protocol request: uri=`{uri}`, source_resource={source_resource:?}");
    match registry.read_protocol_asset(source_resource, authority, uri.path()) {
        Ok((body, mime)) => {
            log::debug!(
                "Web UI custom-protocol response: uri=`{uri}`, status=200, mime={mime}, bytes={}",
                body.len()
            );
            wry::http::Response::builder()
                .header("Content-Type", mime)
                .body(Cow::Owned(body))
                .expect("valid HTTP response")
        }
        Err(error) => {
            log::warn!("Web UI custom-protocol response: uri=`{uri}`, status=404, error={error}");
            wry::http::Response::builder()
                .status(404)
                .header("Content-Type", "text/plain; charset=utf-8")
                .body(Cow::Owned(error.to_string().into_bytes()))
                .expect("valid HTTP response")
        }
    }
}
