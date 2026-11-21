//! Benchmarks for SQLite persistence operations.

use criterion::{criterion_group, criterion_main, BatchSize, Criterion};
use flashcron::config::JobExecution;
use flashcron::db::DatabaseManager;
use std::hint::black_box;
use tempfile::TempDir;
use tokio::runtime::Runtime;

fn database(runtime: &Runtime, temp_dir: &TempDir) -> DatabaseManager {
    let path = temp_dir.path().join("flashcron.sqlite");
    runtime
        .block_on(DatabaseManager::init(path.to_str().unwrap()))
        .unwrap()
}

fn execution(job_name: String) -> JobExecution {
    let mut execution = JobExecution::new(&job_name, "manual");
    execution.complete_success(0, Some("benchmark output\n".repeat(8)));
    execution
}

fn seed_history(runtime: &Runtime, db: &DatabaseManager, count: usize) {
    for index in 0..count {
        let exec = execution(format!("job-{index}"));
        runtime.block_on(db.save(exec, 100, 10000)).unwrap();
    }
}

fn bench_sqlite(c: &mut Criterion) {
    let runtime = Runtime::new().unwrap();

    c.bench_function("sqlite_save_history_and_stats", |b| {
        b.iter_batched(
            || {
                let temp_dir = tempfile::tempdir().unwrap();
                let db = database(&runtime, &temp_dir);
                (db, execution("bench-job".to_owned()), temp_dir)
            },
            |(db, exec, temp_dir)| {
                black_box(runtime.block_on(db.save(exec, 100, 10000)).unwrap());
                (db, temp_dir)
            },
            BatchSize::PerIteration,
        );
    });

    let temp_dir = tempfile::tempdir().unwrap();
    let db = database(&runtime, &temp_dir);
    seed_history(&runtime, &db, 1_000);

    c.bench_function("sqlite_list_recent_100_of_1000", |b| {
        b.iter(|| black_box(runtime.block_on(db.list("", 100)).unwrap()));
    });
}

criterion_group!(benches, bench_sqlite);
criterion_main!(benches);
