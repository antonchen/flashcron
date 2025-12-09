//! Benchmarks for the scheduler

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use flashcron::{Config, Scheduler};
use std::path::PathBuf;

/// Benchmark scheduler initialization with varying job counts
fn bench_scheduler_init(c: &mut Criterion) {
    let mut group = c.benchmark_group("scheduler_init");

    for job_count in [10, 100, 500, 1000].iter() {
        group.bench_with_input(
            BenchmarkId::new("jobs", job_count),
            job_count,
            |b, &count| {
                let config = generate_config(count);
                b.iter(|| {
                    let config = config.clone();
                    let (scheduler, _handle) = Scheduler::new(config, PathBuf::from("bench.toml"));
                    black_box(scheduler)
                });
            },
        );
    }

    group.finish();
}

/// Benchmark config parsing
fn bench_config_parse(c: &mut Criterion) {
    let mut group = c.benchmark_group("config_parse");

    for job_count in [10, 100, 500].iter() {
        let config_str = generate_config_string(*job_count);

        group.bench_with_input(
            BenchmarkId::new("jobs", job_count),
            &config_str,
            |b, config_str| {
                b.iter(|| Config::from_str(black_box(config_str), "bench.toml").unwrap());
            },
        );
    }

    group.finish();
}

/// Benchmark next run calculation
fn bench_next_run(c: &mut Criterion) {
    let config_str = r#"
        [jobs.test]
        schedule = "*/5 * * * *"
        command = "echo test"
    "#;

    let config = Config::from_str(config_str, "test.toml").unwrap();
    let job = config.get_job("test").unwrap();

    c.bench_function("next_run_calculation", |b| {
        b.iter(|| black_box(job.next_run()));
    });
}

/// Generate a config with N jobs
fn generate_config(job_count: usize) -> Config {
    let config_str = generate_config_string(job_count);
    Config::from_str(&config_str, "bench.toml").unwrap()
}

/// Generate config string with N jobs
fn generate_config_string(job_count: usize) -> String {
    let mut config = String::from("[settings]\nmax_concurrent_jobs = 100\n\n");

    for i in 0..job_count {
        config.push_str(&format!(
            r#"[jobs.job_{i}]
schedule = "*/5 * * * *"
command = "echo job {i}"
description = "Benchmark job {i}"

"#,
            i = i
        ));
    }

    config
}

criterion_group!(
    benches,
    bench_scheduler_init,
    bench_config_parse,
    bench_next_run,
);

criterion_main!(benches);
