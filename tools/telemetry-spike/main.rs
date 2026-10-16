//! Phase 0.5 spike: send a handful of synthetic telemetry events to Azure
//! Application Insights and verify they arrive intact.
//!
//! Run with:
//!   AL_CH_SPIKE_CONNECTION_STRING="InstrumentationKey=...;IngestionEndpoint=..." \
//!     cargo run --bin telemetry-spike --release

use std::env;

fn main() {
    env_logger::Builder::new()
        .filter_level(log::LevelFilter::Info)
        .init();

    let cs = env::var("AL_CH_SPIKE_CONNECTION_STRING")
        .expect("set AL_CH_SPIKE_CONNECTION_STRING to run the spike");

    log::info!("Spike: initializing exporter against connection string");
    spike::run(&cs);
}

mod spike {
    use opentelemetry::{
        KeyValue, global,
        trace::{Span, Tracer, TracerProvider as _},
    };
    use opentelemetry_application_insights::Exporter;
    use opentelemetry_sdk::trace::SdkTracerProvider;
    use std::time::Duration;

    pub fn run(connection_string: &str) {
        let http_client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(10))
            .connect_timeout(Duration::from_secs(5))
            .build()
            .unwrap_or_else(|_| reqwest::blocking::Client::new());

        // The spike runs synchronously without a tokio runtime, so it stays
        // on the SimpleSpanProcessor path. Timeouts on the HTTP client are
        // still applied so a hung connection can't wedge the spike.
        let exporter = Exporter::new_from_connection_string(connection_string, http_client)
            .expect("valid connection string");
        let tracer_provider = SdkTracerProvider::builder()
            .with_simple_exporter(exporter)
            .build();

        global::set_tracer_provider(tracer_provider.clone());
        let tracer = tracer_provider.tracer("al-call-hierarchy-spike");

        // 1. Synthetic resolution-miss event
        let mut span = tracer.start("resolution.procedure_not_found");
        span.set_attribute(KeyValue::new("telemetry.alch.failure", "ProcedureNotFound"));
        span.set_attribute(KeyValue::new(
            "telemetry.alch.callee_object_type",
            "Codeunit",
        ));
        span.set_attribute(KeyValue::new(
            "telemetry.alch.callee_source",
            "AppDependency",
        ));
        span.set_attribute(KeyValue::new(
            "telemetry.alch.object_hash",
            "deadbeefcafef00d1234567890abcdef",
        ));
        span.set_attribute(KeyValue::new(
            "telemetry.alch.procedure_hash",
            "0123456789abcdefdeadbeefcafef00d",
        ));
        span.set_attribute(KeyValue::new("telemetry.alch.arg_count", 2_i64));
        span.set_attribute(KeyValue::new("telemetry.alch.schema_version", 1_i64));
        drop(span);

        // 2. Synthetic session.summary (large attribute set)
        let mut sp = tracer.start("session.summary");
        sp.set_attribute(KeyValue::new("telemetry.alch.duration_secs", 600_i64));
        sp.set_attribute(KeyValue::new("telemetry.alch.queue_full_drops", 0_i64));
        sp.set_attribute(KeyValue::new("telemetry.alch.dedup_suppressed", 47_i64));
        sp.set_attribute(KeyValue::new("telemetry.alch.export_attempts", 12_i64));
        sp.set_attribute(KeyValue::new("telemetry.alch.export_failures", 0_i64));
        for i in 0..14 {
            sp.set_attribute(KeyValue::new(
                format!("telemetry.alch.observed.{}", i),
                (i * 7) as i64,
            ));
            sp.set_attribute(KeyValue::new(
                format!("telemetry.alch.exported.{}", i),
                (i * 5) as i64,
            ));
        }
        drop(sp);

        // 3. Burst of 100 summary events to test backend sampling
        for i in 0..100 {
            let mut s = tracer.start("session.summary.burst");
            s.set_attribute(KeyValue::new("burst_seq", i as i64));
            drop(s);
        }

        log::info!("Spike: spans emitted, flushing");
        if let Err(e) = tracer_provider.force_flush() {
            log::error!("flush error: {:?}", e);
        }
        std::thread::sleep(Duration::from_secs(2));
        log::info!("Spike: done. Check App Insights for arrived events.");
    }
}
