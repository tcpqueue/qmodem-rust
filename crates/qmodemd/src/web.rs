use axum::{
    body::Body,
    http::{HeaderMap, header},
    response::Response,
};
const HTML: &[u8] = include_bytes!("../../../frontend/dist/index.html");
const GZIP: &[u8] = include_bytes!("../../../frontend/dist/index.html.gz");
const CSP: &str = include_str!("../../../frontend/dist/csp.txt");
fn accepts_gzip(headers: &HeaderMap) -> bool {
    headers
        .get(header::ACCEPT_ENCODING)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|value| {
            value.split(',').any(|encoding| {
                let mut parts = encoding.trim().split(';');
                parts
                    .next()
                    .is_some_and(|v| v.trim().eq_ignore_ascii_case("gzip"))
                    && parts.all(|p| {
                        let p = p.trim();
                        !p.starts_with("q=")
                            || p[2..].parse::<f32>().is_ok_and(|q| q > 0.0 && q <= 1.0)
                    })
            })
        })
}
pub async fn index(headers: HeaderMap) -> Response {
    let compressed = accepts_gzip(&headers);
    let mut response = Response::builder()
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .header(header::CONTENT_SECURITY_POLICY, CSP)
        .header(header::VARY, "Accept-Encoding")
        .header(header::REFERRER_POLICY, "no-referrer");
    if compressed {
        response = response.header(header::CONTENT_ENCODING, "gzip");
    }
    response
        .body(Body::from(if compressed { GZIP } else { HTML }))
        .expect("static web response")
}
/// License text ships inside the binary alongside the embedded frontend.
pub async fn licenses() -> String {
    format!(
        "{}\n\n{}\n\n{}",
        include_str!("../../../NOTICE.md"),
        include_str!("../../../licenses/art-design-pro.txt"),
        include_str!("../../../licenses/frontend-dependencies.txt")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn embedded_ui_has_csp_and_obeys_gzip_preference() {
        for (encoding, compressed) in [
            ("gzip", true),
            ("br, gzip;q=0.5", true),
            ("gzip;q=0", false),
            ("br", false),
        ] {
            let mut headers = HeaderMap::new();
            headers.insert(header::ACCEPT_ENCODING, encoding.parse().unwrap());
            let response = index(headers).await;
            assert_eq!(
                response.headers().contains_key(header::CONTENT_ENCODING),
                compressed
            );
            assert!(
                response.headers()[header::CONTENT_SECURITY_POLICY]
                    .to_str()
                    .unwrap()
                    .contains("script-src 'sha256-")
            );
            let body = axum::body::to_bytes(response.into_body(), 2 * 1024 * 1024)
                .await
                .unwrap();
            assert_eq!(body.as_ref(), if compressed { GZIP } else { HTML });
        }
    }
}
