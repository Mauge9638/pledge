use std::time::Duration;

use lazy_static::lazy_static;
use prometheus::{Counter, Gauge, Histogram, HistogramOpts, Registry};

lazy_static! {
    pub static ref REGISTRY: Registry = Registry::new();

    // Cache metrics
    pub static ref CACHE_HITS: Counter = Counter::new(
        "pledge_cache_hits_total",
        "Total number of cache hits"
    ).unwrap();

    pub static ref CACHE_MISSES: Counter = Counter::new(
        "pledge_cache_misses_total",
        "Total number of cache misses"
    ).unwrap();

    pub static ref UNCONFIGURED_QUERIES: Counter = Counter::new(
         "pledge_unconfigured_queries_total",
         "Queries without cache templates - consider adding to config"
     ).unwrap();

    pub static ref CACHE_SIZE: Gauge = Gauge::new(
        "pledge_cache_entries",
        "Number of entries in cache",
    ).unwrap();

    pub static ref CACHE_MEMORY_BYTES: Gauge = Gauge::new(
        "pledge_cache_memory_bytes",
        "Bytes used by cache"
    ).unwrap();

    pub static ref CACHE_HIT_QUERY_DURATION: Histogram = Histogram::with_opts(
        HistogramOpts::new(
            "pledge_cache_hit_duration_seconds",
            "End-to-end latency for queries served from cache (postcard deserialize + JSON serialize)"
        )
        .buckets(vec![0.0005, 0.001, 0.002, 0.005, 0.01, 0.05, 0.1, 0.5])
    ).unwrap();

    pub static ref CACHE_CONFIGURED_MISS_QUERY_DURATION: Histogram = Histogram::with_opts(
        HistogramOpts::new(
            "pledge_cache_miss_duration_seconds",
            "Latency for configured queries not yet cached (includes DB query + cache write)"
        )
        .buckets(vec![0.05, 0.1, 0.25, 0.5, 1.0, 2.0, 5.0])
    ).unwrap();

    pub static ref TOTAL_QUERY_DURATION: Histogram = Histogram::with_opts(
        HistogramOpts::new(
            "pledge_total_query_duration_seconds",
            "The total query duration, meaning how long all queries have taken, including cache hits and misses."
        )
        .buckets(vec![0.0005, 0.001, 0.002, 0.005, 0.01, 0.05, 0.1, 0.5])
    ).unwrap();
}

pub fn register_metrics() {
    REGISTRY.register(Box::new(CACHE_HITS.clone())).unwrap();
    REGISTRY.register(Box::new(CACHE_MISSES.clone())).unwrap();
    REGISTRY.register(Box::new(CACHE_SIZE.clone())).unwrap();
    REGISTRY
        .register(Box::new(CACHE_MEMORY_BYTES.clone()))
        .unwrap();
    REGISTRY
        .register(Box::new(TOTAL_QUERY_DURATION.clone()))
        .unwrap();
}

pub fn register_cache_hit(elapsed: Duration) {
    println!("Cache hit duration: {} seconds", elapsed.as_secs_f64());
}
