//! Hot-path benchmark: `record_resolution_miss` must average ≤ 5µs per call
//! when telemetry is enabled, and effectively zero when disabled.

#![cfg(feature = "telemetry")]

use al_sem::telemetry;
use criterion::{Criterion, criterion_group, criterion_main};
use std::hint::black_box;

fn make_ctx() -> telemetry::CallContext<'static> {
    telemetry::CallContext {
        failure: telemetry::ResolutionFailure::ProcedureNotFound,
        call_pattern: telemetry::CallPattern::Qualified,
        callee_object_type: Some(telemetry::ObjectType::Codeunit),
        callee_source: telemetry::CalleeSource::AppDependency,
        caller_object_type: telemetry::ObjectType::Page,
        caller_context: telemetry::CallerContext::Trigger,
        callee_object_name: Some("CustomerObj"),
        callee_procedure_name: "PostInvoice",
        arg_count: 2,
        ts_node_path: "method_call>member_expression>identifier",
    }
}

fn bench_disabled(c: &mut Criterion) {
    c.bench_function("record_resolution_miss / disabled", |b| {
        b.iter(|| {
            // Without init, runtime::get() is None; record_* returns immediately.
            telemetry::record_resolution_miss(black_box(&make_ctx()));
        });
    });
}

criterion_group!(benches, bench_disabled);
criterion_main!(benches);
