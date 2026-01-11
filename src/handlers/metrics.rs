use crate::metrics::REGISTRY;
use axum::http::{Response, header};
use prometheus::TextEncoder;

pub async fn metrics_handler() -> Response<String> {
    println!("Metrics getting called");
    let encoder = TextEncoder::new();
    let metric_families = REGISTRY.gather();
    let mut buffer = String::new();
    encoder.encode_utf8(&metric_families, &mut buffer).unwrap();

    Response::builder()
        .header(header::CONTENT_TYPE, "text/plain")
        .body(buffer)
        .unwrap()
}
