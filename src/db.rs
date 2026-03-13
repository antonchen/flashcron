//! SQLite database persistence layer

use crate::config::{JobExecution, JobStatus};
use crate::error::{Error, Result};
use rusqlite::params;
use std::collections::HashMap;
use tokio_rusqlite::Connection;
use uuid::Uuid;

/// Database manager for SQLite persistence
#[derive(Clone)]
pub struct DatabaseManager {
    conn: Connection,
}

impl DatabaseManager {
    /// Initialize the database and create tables
    pub async fn init(path: &str) -> Result<Self> {
        let conn = Connection::open(path)
            .await
            .map_err(|e| Error::Config(format!("Failed to open database: {}", e)))?;

        conn.call(|conn| {
            // 1. Job History & Stats
            conn.execute(
                "CREATE TABLE IF NOT EXISTS job_history (
                    execution_id TEXT PRIMARY KEY,
                    job_name TEXT NOT NULL,
                    trigger_source TEXT NOT NULL,
                    start_time TEXT NOT NULL,
                    duration_ms INTEGER,
                    exit_code INTEGER,
                    output TEXT
                )",
                [],
            )?;

            conn.execute(
                "CREATE TABLE IF NOT EXISTS job_stats (
                    job_name TEXT PRIMARY KEY,
                    success_count INTEGER DEFAULT 0,
                    failure_count INTEGER DEFAULT 0
                )",
                [],
            )?;

            // 2. Package Manager Tables (for GEMINI_pkg.md)
            conn.execute(
                "CREATE TABLE IF NOT EXISTS managed_packages (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    pkg_type TEXT NOT NULL, -- 'apt' or 'pip'
                    name TEXT NOT NULL,
                    status INTEGER DEFAULT 1, -- 1: Installed, 0: Failed
                    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                    UNIQUE(pkg_type, name)
                )",
                [],
            )?;

            conn.execute(
                "CREATE TABLE IF NOT EXISTS system_config (
                    config_key TEXT PRIMARY KEY,
                    config_value TEXT NOT NULL
                )",
                [],
            )?;

            // Indexes for performance
            conn.execute(
                "CREATE INDEX IF NOT EXISTS idx_history_job_name ON job_history(job_name)",
                [],
            )?;
            conn.execute(
                "CREATE INDEX IF NOT EXISTS idx_history_start_time ON job_history(start_time DESC)",
                [],
            )?;

            Ok(())
        })
        .await
        .map_err(|e| Error::Config(format!("Database init failed: {}", e)))?;

        Ok(Self { conn })
    }

    /// Save execution results to history and update stats
    pub async fn save(&self, exec: JobExecution) -> Result<()> {
        let execution_id = exec.id.to_string();
        let job_name = exec.job_name.clone();
        let trigger = exec.trigger.clone();
        let start_time = exec.started_at.to_rfc3339();
        let duration = exec.duration().map(|d| d.num_milliseconds()).unwrap_or(0);
        let exit_code = exec.exit_code;

        // Combine stdout and stderr for persistent storage
        let output = format!(
            "{}{}",
            exec.stdout.as_deref().unwrap_or(""),
            exec.stderr.as_deref().unwrap_or("")
        );

        let is_success = matches!(exec.status, JobStatus::Success);

        self.conn.call(move |conn| {
            let tx = conn.transaction()?;

            // Insert into history
            tx.execute(
                "INSERT INTO job_history (execution_id, job_name, trigger_source, start_time, duration_ms, exit_code, output)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![execution_id, job_name, trigger, start_time, duration, exit_code, output],
            )?;

            // Upsert into stats
            tx.execute(
                "INSERT INTO job_stats (job_name, success_count, failure_count)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(job_name) DO UPDATE SET
                    success_count = success_count + ?2,
                    failure_count = failure_count + ?3",
                params![
                    job_name,
                    if is_success { 1 } else { 0 },
                    if is_success { 0 } else { 1 }
                ],
            )?;

            tx.commit()?;
            Ok(())
        })
        .await
        .map_err(|e| Error::Config(format!("Failed to save execution: {}", e)))?;

        Ok(())
    }

    /// List recent history (metadata only). If job_name is empty, returns global history.
    pub async fn list(&self, job_name: &str, limit: usize) -> Result<Vec<JobExecution>> {
        let job_name = job_name.to_string();

        self.conn
            .call(move |conn| {
                if job_name.is_empty() {
                    let mut stmt = conn.prepare(
                        "SELECT execution_id, job_name, trigger_source, start_time, duration_ms, exit_code, '' as output
                         FROM job_history
                         ORDER BY start_time DESC
                         LIMIT ?1",
                    )?;
                    let rows = stmt.query_map(params![limit], Self::map_row_to_execution)?;
                    let mut results = Vec::new();
                    for row in rows {
                        results.push(row?);
                    }
                    Ok(results)
                } else {
                    let mut stmt = conn.prepare(
                        "SELECT execution_id, job_name, trigger_source, start_time, duration_ms, exit_code, '' as output
                         FROM job_history
                         WHERE job_name = ?1
                         ORDER BY start_time DESC
                         LIMIT ?2",
                    )?;
                    let rows = stmt.query_map(params![job_name, limit], Self::map_row_to_execution)?;
                    let mut results = Vec::new();
                    for row in rows {
                        results.push(row?);
                    }
                    Ok(results)
                }
            })
            .await
            .map_err(|e| Error::Config(format!("Failed to list history: {}", e)))
    }

    /// Helper to map a database row to a JobExecution
    fn map_row_to_execution(row: &rusqlite::Row) -> rusqlite::Result<JobExecution> {
        let id_str: String = row.get(0)?;
        let id = Uuid::parse_str(&id_str).unwrap_or_else(|_| Uuid::nil());
        let started_at_str: String = row.get(3)?;
        let started_at = chrono::DateTime::parse_from_rfc3339(&started_at_str)
            .map(|dt| dt.with_timezone(&chrono::Utc))
            .unwrap_or_else(|_| chrono::Utc::now());

        let duration_ms: i64 = row.get(4)?;
        let ended_at = started_at + chrono::Duration::milliseconds(duration_ms);
        let exit_code: Option<i32> = row.get(5)?;

        Ok(JobExecution {
            id,
            job_name: row.get(1)?,
            trigger: row.get(2)?,
            started_at,
            ended_at: Some(ended_at),
            status: if exit_code == Some(0) {
                JobStatus::Success
            } else {
                JobStatus::Failed {
                    error: "Completed".into(),
                }
            },
            exit_code,
            stdout: None,
            stderr: None,
            attempt: 1,
        })
    }

    /// Get full execution details including output
    pub async fn get(&self, execution_id: Uuid) -> Result<Option<JobExecution>> {
        let id_str = execution_id.to_string();

        self.conn.call(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT execution_id, job_name, trigger_source, start_time, duration_ms, exit_code, output
                 FROM job_history
                 WHERE execution_id = ?1",
            )?;

            let mut rows = stmt.query_map(params![id_str], |row| {
                let mut exec = Self::map_row_to_execution(row)?;
                let output: String = row.get(6)?;
                exec.stdout = Some(output);
                Ok(exec)
            })?;

            if let Some(row) = rows.next() {
                Ok(Some(row?))
            } else {
                Ok(None)
            }
        })
        .await
        .map_err(|e| Error::Config(format!("Failed to get execution: {}", e)))
    }

    /// Load all job statistics for state recovery
    pub async fn load_stats(&self) -> Result<HashMap<String, (u64, u64)>> {
        self.conn
            .call(|conn| {
                let mut stmt =
                    conn.prepare("SELECT job_name, success_count, failure_count FROM job_stats")?;
                let rows = stmt.query_map([], |row| {
                    let name: String = row.get(0)?;
                    let success: i64 = row.get(1)?;
                    let failure: i64 = row.get(2)?;
                    Ok((name, (success as u64, failure as u64)))
                })?;

                let mut stats = HashMap::new();
                for row in rows {
                    let (name, s) = row?;
                    stats.insert(name, s);
                }
                Ok(stats)
            })
            .await
            .map_err(|e| Error::Config(format!("Failed to load stats: {}", e)))
    }

    /// Perform maintenance: per-job limit, total limit, and orphan cleanup
    pub async fn cleanup(
        &self,
        active_jobs: Vec<String>,
        job_history_size: usize,
        max_history_size: usize,
    ) -> Result<()> {
        let active_jobs_inner = active_jobs.clone();

        self.conn
            .call(move |conn| {
                let tx = conn.transaction()?;

                // 1. Per-job limit: keep only last N for each active job
                for name in &active_jobs_inner {
                    tx.execute(
                        "DELETE FROM job_history WHERE job_name = ?1 AND execution_id NOT IN (
                        SELECT execution_id FROM (
                            SELECT execution_id FROM job_history 
                            WHERE job_name = ?1 
                            ORDER BY start_time DESC 
                            LIMIT ?2
                        )
                    )",
                        params![name, job_history_size],
                    )?;
                }

                // 2. Global limit: max_history_size (use a very large number for LIMIT)
                tx.execute(
                    "DELETE FROM job_history WHERE execution_id IN (
                    SELECT execution_id FROM (
                        SELECT execution_id FROM job_history 
                        ORDER BY start_time DESC 
                        LIMIT 999999999 OFFSET ?1
                    )
                )",
                    params![max_history_size],
                )?;

                // 3. Orphan cleanup: delete history and stats for jobs not in current config
                if !active_jobs_inner.is_empty() {
                    let placeholders = active_jobs_inner
                        .iter()
                        .map(|_| "?")
                        .collect::<Vec<_>>()
                        .join(",");

                    let sql_stats = format!(
                        "DELETE FROM job_stats WHERE job_name NOT IN ({})",
                        placeholders
                    );
                    tx.execute(
                        &sql_stats,
                        rusqlite::params_from_iter(active_jobs_inner.iter()),
                    )?;

                    let sql_history = format!(
                        "DELETE FROM job_history WHERE job_name NOT IN ({})",
                        placeholders
                    );
                    tx.execute(
                        &sql_history,
                        rusqlite::params_from_iter(active_jobs_inner.iter()),
                    )?;
                }

                tx.commit()?;
                Ok(())
            })
            .await
            .map_err(|e| Error::Config(format!("Maintenance failed: {}", e)))?;

        Ok(())
    }

    // --- Package Manager Methods ---

    /// Get all packages that should be installed
    pub async fn get_packages(&self) -> Result<Vec<(String, String)>> {
        self.conn
            .call(|conn| {
                let mut stmt =
                    conn.prepare("SELECT pkg_type, name FROM managed_packages WHERE status = 1")?;
                let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
                let mut res = Vec::new();
                for r in rows {
                    res.push(r?);
                }
                Ok(res)
            })
            .await
            .map_err(|e| Error::Config(format!("Failed to get packages: {}", e)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    async fn setup_db() -> (DatabaseManager, NamedTempFile) {
        let file = NamedTempFile::new().unwrap();
        let path = file.path().to_str().unwrap();
        let db = DatabaseManager::init(path).await.unwrap();
        (db, file)
    }

    fn mock_execution(name: &str, success: bool) -> JobExecution {
        let mut exec = JobExecution::new(name, "manual");
        if success {
            exec.complete_success(0, "stdout content".into(), "stderr content".into());
        } else {
            exec.complete_failed("Error".into(), Some(1), "".into(), "error log".into());
        }
        exec
    }

    #[tokio::test]
    async fn test_db_init_and_save() {
        let (db, _temp) = setup_db().await;
        let exec = mock_execution("test-job", true);
        let id = exec.id;

        db.save(exec).await.unwrap();

        let details = db.get(id).await.unwrap().unwrap();
        assert_eq!(details.job_name, "test-job");
        assert_eq!(details.stdout, Some("stdout contentstderr content".into()));
    }

    #[tokio::test]
    async fn test_db_stats_recovery() {
        let (db, _temp) = setup_db().await;

        db.save(mock_execution("job-1", true)).await.unwrap();
        db.save(mock_execution("job-1", true)).await.unwrap();
        db.save(mock_execution("job-1", false)).await.unwrap();

        let stats = db.load_stats().await.unwrap();
        assert_eq!(stats.get("job-1"), Some(&(2, 1)));
    }

    #[tokio::test]
    async fn test_db_cleanup_respects_limits() {
        let (db, _temp) = setup_db().await;

        // Save 5 executions
        for _ in 0..5 {
            db.save(mock_execution("job-1", true)).await.unwrap();
        }

        // Cleanup: keep only 2 per job
        db.cleanup(vec!["job-1".into()], 2, 10000).await.unwrap();

        let history = db.list("job-1", 10).await.unwrap();
        assert_eq!(history.len(), 2);
    }
}
