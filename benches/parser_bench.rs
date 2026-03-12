//! Benchmarks for cron expression parsing

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use cron::Schedule;
use std::str::FromStr;

/// Benchmark parsing various cron expressions
fn bench_cron_parse(c: &mut Criterion) {
    let mut group = c.benchmark_group("cron_parse");

    let expressions = vec![
        ("simple", "0 * * * * *"),
        ("every_5min", "0 */5 * * * *"),
        ("complex", "0 30 4 1,15 * 1-5"),
        ("ranges", "0 0-30/5 9-17 * * 1-5"),
    ];

    for (name, expr) in expressions {
        group.bench_with_input(BenchmarkId::new("expression", name), &expr, |b, &expr| {
            b.iter(|| Schedule::from_str(black_box(expr)).unwrap());
        });
    }

    group.finish();
}

/// Benchmark next occurrence calculation
fn bench_next_occurrence(c: &mut Criterion) {
    let mut group = c.benchmark_group("next_occurrence");

    let expressions = vec![
        ("every_minute", "0 * * * * *"),
        ("every_5min", "0 */5 * * * *"),
        ("daily", "0 0 0 * * *"),
        ("weekly", "0 0 0 * * 7"),
    ];

    for (name, expr) in expressions {
        let schedule = Schedule::from_str(expr).unwrap();

        group.bench_with_input(
            BenchmarkId::new("schedule", name),
            &schedule,
            |b, schedule| {
                b.iter(|| {
                    let mut iter = schedule.upcoming(chrono::Utc);
                    black_box(iter.next())
                });
            },
        );
    }

    group.finish();
}

/// Benchmark multiple next occurrences
fn bench_multiple_occurrences(c: &mut Criterion) {
    let schedule = Schedule::from_str("0 */5 * * * *").unwrap();

    c.bench_function("next_10_occurrences", |b| {
        b.iter(|| {
            let occurrences: Vec<_> = schedule.upcoming(chrono::Utc).take(10).collect();
            black_box(occurrences)
        });
    });

    c.bench_function("next_100_occurrences", |b| {
        b.iter(|| {
            let occurrences: Vec<_> = schedule.upcoming(chrono::Utc).take(100).collect();
            black_box(occurrences)
        });
    });
}

/// Benchmark TOML parsing
fn bench_toml_parse(c: &mut Criterion) {
    let config_small = format!(
        r#"
        [settings]
        log_level = "{}"

        [jobs.test]
        schedule = "* * * * *"
        command = "echo test"
    "#,
        flashcron::config::DEFAULT_LOG_LEVEL
    );

    let config_medium = generate_config(50);
    let config_large = generate_config(200);

    let mut group = c.benchmark_group("toml_parse");

    group.bench_function("small", |b| {
        b.iter(|| toml::from_str::<toml::Value>(black_box(&config_small)).unwrap());
    });

    group.bench_function("medium_50_jobs", |b| {
        b.iter(|| toml::from_str::<toml::Value>(black_box(&config_medium)).unwrap());
    });

    group.bench_function("large_200_jobs", |b| {
        b.iter(|| toml::from_str::<toml::Value>(black_box(&config_large)).unwrap());
    });

    group.finish();
}

fn generate_config(job_count: usize) -> String {
    let mut config = format!(
        "[settings]\nlog_level = \"{}\"\n\n",
        flashcron::config::DEFAULT_LOG_LEVEL
    );

    for i in 0..job_count {
        config.push_str(&format!(
            r#"[jobs.job_{i}]
schedule = "*/5 * * * *"
command = "echo job {i}"
description = "Test job {i}"
timeout = 60
retry_count = 3

"#,
            i = i
        ));
    }

    config
}

criterion_group!(
    benches,
    bench_cron_parse,
    bench_next_occurrence,
    bench_multiple_occurrences,
    bench_toml_parse,
);

criterion_main!(benches);
