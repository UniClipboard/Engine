use std::time::Duration;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use uc_infra::db::repositories::GroupUpdateDeliveryBenchmark;
use uc_infra::space::AdmissionRepositoryBenchmark;

fn benchmark_sizes() -> &'static [usize] {
    if std::env::var_os("UNICLIPBOARD_BENCH_SMOKE").is_some() {
        &[0, 1024 * 1024]
    } else {
        &[0, 1024 * 1024, 7 * 1024 * 1024, 25 * 1024 * 1024]
    }
}

fn admission_repository(c: &mut Criterion) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|error| panic!("benchmark runtime failed: {error}"));
    let mut group = c.benchmark_group("admission_repository/no_current_join/cold");
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(3));

    for &unrelated_record_bytes in benchmark_sizes() {
        let fixture = AdmissionRepositoryBenchmark::without_current_join(unrelated_record_bytes)
            .unwrap_or_else(|error| panic!("benchmark fixture failed: {error:#}"));
        group.bench_with_input(
            BenchmarkId::from_parameter(unrelated_record_bytes),
            &fixture,
            |b, fixture| {
                b.to_async(&runtime).iter(|| async {
                    fixture
                        .load_without_current_join()
                        .await
                        .unwrap_or_else(|error| panic!("benchmark load failed: {error:#}"));
                });
            },
        );
    }
    group.finish();
}

fn group_update_delivery(c: &mut Criterion) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|error| panic!("benchmark runtime failed: {error}"));
    let mut group = c.benchmark_group("group_update_delivery/warm_due");
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(3));

    for &material_bytes in benchmark_sizes() {
        let fixture = runtime
            .block_on(GroupUpdateDeliveryBenchmark::new(material_bytes))
            .unwrap_or_else(|error| panic!("benchmark fixture failed: {error:#}"));
        group.bench_with_input(
            BenchmarkId::from_parameter(material_bytes),
            &fixture,
            |b, fixture| {
                b.to_async(&runtime).iter(|| async {
                    fixture
                        .load_due()
                        .await
                        .unwrap_or_else(|error| panic!("benchmark load failed: {error:#}"));
                });
            },
        );
    }
    group.finish();
}

criterion_group!(benches, admission_repository, group_update_delivery);
criterion_main!(benches);
