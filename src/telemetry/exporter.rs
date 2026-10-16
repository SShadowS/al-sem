//! Background-thread exporter wrapping `opentelemetry-application-insights`.
//!
//! Owns the tokio current-thread runtime, the OTel SDK pipeline, and the
//! receiver end of the mpsc channel. Constructs and exports `session.summary`
//! at shutdown after queue drain.

use crate::telemetry::counters::Counters;
use crate::telemetry::events::{EventEnvelope, EventKind, SessionSummary};
use opentelemetry::{
    KeyValue, global,
    trace::{Span, Tracer, TracerProvider as _},
};
use opentelemetry_application_insights::Exporter;
use opentelemetry_sdk::{Resource, trace::SdkTracerProvider};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::Receiver;

/// The Application Insights `cloud_RoleName` every event is filed under.
///
/// This deliberately still reads `al-call-hierarchy` after the 2026-08-07 crate rename.
/// The service name is the join key for existing App Insights dashboards and saved
/// queries; changing it would split every one of them at the rename point, and would buy
/// nothing a user can see. Do not "fix" it for consistency with the crate name — that is
/// a decision with a cost outside this repository.
const SERVICE_NAME: &str = "al-call-hierarchy";

pub struct ExporterConfig {
    pub connection_string: String,
    pub flush_interval: Duration,
    pub batch_size: u32,
}

/// Spawns a dedicated OS thread hosting a current-thread tokio runtime and
/// runs the exporter loop. Returns a join handle the caller awaits at shutdown.
pub fn spawn(
    config: ExporterConfig,
    rx: Receiver<EventEnvelope>,
    counters: Arc<Counters>,
    started_at: Instant,
) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("al-ch-telemetry".to_string())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio current-thread runtime");
            rt.block_on(run(config, rx, counters, started_at));
        })
        .expect("spawn telemetry thread")
}

/// The exporter's HTTP client. reqwest is built `rustls-no-provider` (see
/// Cargo.toml), so `ring` is installed as the process crypto provider before the
/// first client; `Err` means another provider is already installed, which serves
/// as well.
fn http_client() -> reqwest::blocking::Client {
    let _ = rustls::crypto::ring::default_provider().install_default();
    reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .connect_timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap_or_else(|_| reqwest::blocking::Client::new())
}

async fn run(
    config: ExporterConfig,
    mut rx: Receiver<EventEnvelope>,
    counters: Arc<Counters>,
    started_at: Instant,
) {
    let _ = config.flush_interval; // Reserved for batch-mode upgrade later.
    let _ = config.batch_size;

    let http_client = http_client();

    let exporter =
        match Exporter::new_from_connection_string(config.connection_string.clone(), http_client) {
            Ok(e) => e,
            Err(e) => {
                log::warn!("telemetry: exporter init failed: {}; subsystem disabled", e);
                return;
            }
        };
    // `service.name` on the resource is what App Insights files as `cloud_RoleName`.
    // The batch processor runs on its own thread, so the blocking HTTP client never
    // runs inside this tokio runtime.
    let tracer_provider = SdkTracerProvider::builder()
        .with_resource(Resource::builder().with_service_name(SERVICE_NAME).build())
        .with_batch_exporter(exporter)
        .build();

    global::set_tracer_provider(tracer_provider.clone());
    let tracer = tracer_provider.tracer(SERVICE_NAME);

    while let Some(env) = rx.recv().await {
        export_event(&tracer, &env, &counters);
    }

    // Channel disconnected → producer side closed → drain done. Build summary.
    let summary = build_session_summary(started_at, &counters);
    export_summary(&tracer, &summary);

    if let Err(e) = tracer_provider.force_flush() {
        log::warn!("telemetry: final flush error: {:?}", e);
    }
}

fn export_event(tracer: &impl Tracer, env: &EventEnvelope, counters: &Counters) {
    counters.export_attempted();
    let leaf = env.event.leaf();
    let label = match &env.event {
        EventKind::ResolutionMiss(_) => "resolution.miss",
        EventKind::ParserError(_) => "parser.error",
        EventKind::HandlerEmpty(_) => "handler.empty_result",
        EventKind::IndexerIssue(_) => "indexer.issue",
        EventKind::SessionStart(_) => "session.start",
        EventKind::SessionSummary(_) => "session.summary",
    };
    let mut span = tracer.start(label);
    span.set_attribute(KeyValue::new(
        "telemetry.alch.schema_version",
        env.schema_version as i64,
    ));
    span.set_attribute(KeyValue::new(
        "telemetry.alch.install_id",
        env.install_id.clone(),
    ));
    span.set_attribute(KeyValue::new(
        "telemetry.alch.workspace_id",
        env.workspace_id.clone(),
    ));
    span.set_attribute(KeyValue::new("telemetry.alch.al_version", env.al_version));
    span.set_attribute(KeyValue::new(
        "telemetry.alch.grammar_version",
        env.grammar_version,
    ));
    span.set_attribute(KeyValue::new("telemetry.alch.os", env.os));
    span.set_attribute(KeyValue::new(
        "telemetry.alch.session_id",
        env.session_id as i64,
    ));
    crate::telemetry::events_attrs::apply(&mut span, &env.event);
    drop(span);
    if let Some(k) = leaf {
        counters.export_succeeded(k);
    }
}

fn export_summary(tracer: &impl Tracer, summary: &SessionSummary) {
    let mut span = tracer.start("session.summary");
    span.set_attribute(KeyValue::new(
        "telemetry.alch.duration_secs",
        summary.duration_secs as i64,
    ));
    span.set_attribute(KeyValue::new(
        "telemetry.alch.unique_patterns",
        summary.unique_patterns as i64,
    ));
    span.set_attribute(KeyValue::new(
        "telemetry.alch.queue_full_drops",
        summary.queue_full_drops as i64,
    ));
    span.set_attribute(KeyValue::new(
        "telemetry.alch.dedup_suppressed",
        summary.dedup_suppressed as i64,
    ));
    span.set_attribute(KeyValue::new(
        "telemetry.alch.export_attempts",
        summary.export_attempts as i64,
    ));
    span.set_attribute(KeyValue::new(
        "telemetry.alch.export_failures",
        summary.export_failures as i64,
    ));
    for (i, v) in summary.observed_by_kind.iter().enumerate() {
        span.set_attribute(KeyValue::new(
            format!("telemetry.alch.observed.{}", i),
            *v as i64,
        ));
    }
    for (i, v) in summary.exported_by_kind.iter().enumerate() {
        span.set_attribute(KeyValue::new(
            format!("telemetry.alch.exported.{}", i),
            *v as i64,
        ));
    }
    drop(span);
}

fn build_session_summary(started_at: Instant, counters: &Counters) -> SessionSummary {
    let snap = counters.snapshot();
    SessionSummary {
        duration_secs: started_at.elapsed().as_secs(),
        unique_patterns: 0, // populated by dedup module if/when wired in
        queue_full_drops: snap.queue_full_drops,
        dedup_suppressed: snap.dedup_suppressed,
        export_attempts: snap.export_attempts,
        export_failures: snap.export_failures,
        observed_by_kind: snap.observed_by_kind,
        exported_by_kind: snap.exported_by_kind,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_summary_pulls_atomics() {
        let c = Counters::new();
        c.queue_full();
        c.queue_full();
        c.dedup_suppress();
        c.observe(crate::telemetry::events::LeafKind::ParserTreeError);
        let summary = build_session_summary(Instant::now(), &c);
        assert_eq!(summary.queue_full_drops, 2);
        assert_eq!(summary.dedup_suppressed, 1);
        assert_eq!(
            summary.observed_by_kind[crate::telemetry::events::LeafKind::ParserTreeError.index()],
            1
        );
    }

    /// The TLS client builds on the `ring` provider. Without the provider install,
    /// reqwest's `rustls-no-provider` client cannot build and `Client::new()`
    /// panics. Mutation: drop the `install_default()` line -> this test panics.
    #[test]
    fn http_client_builds_on_ring() {
        let _client = http_client();
        assert!(rustls::crypto::CryptoProvider::get_default().is_some());
    }
}
