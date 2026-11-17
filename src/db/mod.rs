//! SQLite database persistence layer

#[cfg(feature = "web")]
use crate::config::Job;
use crate::config::{JobExecution, JobStatus};
use crate::error::{Error, Result};
use rusqlite::params;
#[cfg(feature = "web")]
use std::collections::HashMap;
use tokio_rusqlite::Connection;
use uuid::Uuid;

#[cfg(feature = "dep-manager")]
mod packages;

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
            .map_err(|e| Error::Config(e.to_string()))?;

        conn.call(|conn| -> rusqlite::Result<_> {
            // 1. Job History
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

            #[cfg(feature = "web")]
            conn.execute(
                "CREATE TABLE IF NOT EXISTS jobs (
                    name TEXT PRIMARY KEY,
                    definition TEXT NOT NULL
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

            #[cfg(feature = "dep-manager")]
            {
                conn.execute(
                    "CREATE TABLE IF NOT EXISTS packages (
                        id INTEGER PRIMARY KEY AUTOINCREMENT,
                        pkg_type TEXT NOT NULL,
                        name TEXT NOT NULL,
                        status INTEGER NOT NULL,
                        target TEXT,
                        current TEXT,
                        error_msg TEXT,
                        created_at TEXT NOT NULL,
                        updated_at TEXT NOT NULL,
                        UNIQUE(pkg_type, name)
                    )",
                    [],
                )?;
            }

            Ok(())
        })
        .await
        .map_err(|e| Error::Config(e.to_string()))?;

        Ok(Self { conn })
    }

    #[cfg(feature = "web")]
    pub async fn list_jobs(&self) -> Result<HashMap<String, Job>> {
        let rows = self
            .conn
            .call(|conn| -> rusqlite::Result<Vec<(String, String)>> {
                let mut stmt = conn.prepare("SELECT name, definition FROM jobs")?;
                let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
                rows.collect()
            })
            .await
            .map_err(|e| Error::Config(e.to_string()))?;

        rows.into_iter()
            .map(|(name, definition)| {
                let job = serde_json::from_str(&definition)
                    .map_err(|e| Error::Config(format!("Invalid stored job '{name}': {e}")))?;
                Ok((name, job))
            })
            .collect()
    }

    #[cfg(feature = "web")]
    pub async fn store_job(&self, name: String, job: Job, update: bool) -> Result<bool> {
        let definition = serde_json::to_string(&job)
            .map_err(|e| Error::Config(format!("Failed to serialize job: {e}")))?;
        self.conn
            .call(move |conn| -> rusqlite::Result<bool> {
                let changed = if update {
                    conn.execute(
                        "UPDATE jobs SET definition = ?2 WHERE name = ?1",
                        params![name, definition],
                    )?
                } else {
                    conn.execute(
                        "INSERT INTO jobs (name, definition) VALUES (?1, ?2)
                         ON CONFLICT(name) DO NOTHING",
                        params![name, definition],
                    )?
                };
                Ok(changed == 1)
            })
            .await
            .map_err(|e| Error::Config(e.to_string()))
    }

    #[cfg(feature = "web")]
    pub async fn delete_job(&self, name: &str) -> Result<bool> {
        let name = name.to_owned();
        self.conn
            .call(move |conn| -> rusqlite::Result<usize> {
                let tx = conn.unchecked_transaction()?;
                let changed = tx.execute("DELETE FROM jobs WHERE name = ?1", [&name])?;
                if changed == 1 {
                    tx.execute("DELETE FROM job_history WHERE job_name = ?1", [&name])?;
                }
                tx.commit()?;
                Ok(changed)
            })
            .await
            .map(|changed| changed == 1)
            .map_err(|e| Error::Config(e.to_string()))
    }

    /// Save execution results to history
    pub async fn save(&self, exec: JobExecution, persist_history: bool) -> Result<()> {
        let execution_id = exec.id.to_string();
        let job_name = exec.job_name.clone();
        let trigger = exec.trigger.clone();
        let start_time = exec.started_at.to_rfc3339();
        let duration = exec.duration().map(|d| d.num_milliseconds()).unwrap_or(0);
        let exit_code = exec.exit_code;
        let output = exec.output.clone();

        self.conn.call(move |conn| -> rusqlite::Result<_> {
            if persist_history {
                conn.execute(
                    "INSERT INTO job_history (execution_id, job_name, trigger_source, start_time, duration_ms, exit_code, output)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![execution_id, job_name, trigger, start_time, duration, exit_code, output],
                )?;
            }

            Ok(())
        })
        .await
        .map_err(|e| Error::Config(e.to_string()))?;

        Ok(())
    }

    /// List recent history (metadata only). If job_name is empty, returns global history.
    pub async fn list(&self, job_name: &str, limit: usize) -> Result<Vec<JobExecution>> {
        let job_name = job_name.to_string();

        self.conn
            .call(move |conn| -> rusqlite::Result<_> {
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
            .map_err(|e| Error::Config(e.to_string()))
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
            output: None,
            attempt: 1,
        })
    }

    /// Get full execution details including output
    pub async fn get(&self, execution_id: Uuid) -> Result<Option<JobExecution>> {
        let id_str = execution_id.to_string();

        self.conn.call(move |conn| -> rusqlite::Result<_> {
            let mut stmt = conn.prepare(
                "SELECT execution_id, job_name, trigger_source, start_time, duration_ms, exit_code, output
                 FROM job_history
                 WHERE execution_id = ?1",
            )?;

            let mut rows = stmt.query_map(params![id_str], |row| {
                let mut exec = Self::map_row_to_execution(row)?;
                let output: Option<String> = row.get(6)?;
                exec.output = output.filter(|output| !output.is_empty());
                Ok(exec)
            })?;

            if let Some(row) = rows.next() {
                Ok(Some(row?))
            } else {
                Ok(None)
            }
        })
        .await
        .map_err(|e| Error::Config(e.to_string()))
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
            .call(move |conn| -> rusqlite::Result<_> {
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

                // 3. Orphan cleanup: delete history for jobs not in current config
                if !active_jobs_inner.is_empty() {
                    let placeholders = active_jobs_inner
                        .iter()
                        .map(|_| "?")
                        .collect::<Vec<_>>()
                        .join(",");

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
            .map_err(|e| Error::Config(e.to_string()))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    async fn setup_db() -> (DatabaseManager, NamedTempFile) {
        let file = NamedTempFile::new_in("/tmp").unwrap();
        let path = file.path().to_str().unwrap();
        let db = DatabaseManager::init(path).await.unwrap();
        (db, file)
    }

    fn mock_execution(name: &str, success: bool) -> JobExecution {
        let mut exec = JobExecution::new(name, "manual");
        if success {
            exec.complete_success(0, Some("stdout contentstderr content".into()));
        } else {
            exec.complete_failed("Error".into(), Some(1), Some("error log".into()));
        }
        exec
    }

    #[tokio::test]
    async fn test_db_init_and_save() {
        let (db, _temp) = setup_db().await;
        let exec = mock_execution("test-job", true);
        let id = exec.id;

        db.save(exec, true).await.unwrap();

        let details = db.get(id).await.unwrap().unwrap();
        assert_eq!(details.job_name, "test-job");
        assert_eq!(details.output, Some("stdout contentstderr content".into()));
    }

    #[cfg(feature = "web")]
    #[tokio::test]
    async fn test_web_jobs_round_trip() {
        let (db, _temp) = setup_db().await;
        let job = Job {
            schedule: "*/5 * * * *".into(),
            command: "echo persisted".into(),
            ..Job::default()
        };

        assert!(db
            .store_job("sample".into(), job.clone(), false)
            .await
            .unwrap());
        assert!(!db.store_job("sample".into(), job, false).await.unwrap());
        assert_eq!(
            db.list_jobs().await.unwrap()["sample"].command,
            "echo persisted"
        );
    }

    #[tokio::test]
    async fn test_db_skip_history_payload_when_persistence_disabled() {
        let (db, _temp) = setup_db().await;

        db.save(mock_execution("test-job", true), false)
            .await
            .unwrap();

        let history = db.list("test-job", 10).await.unwrap();
        assert!(history.is_empty());
    }

    #[tokio::test]
    async fn test_db_cleanup_respects_limits() {
        let (db, _temp) = setup_db().await;

        // Save 5 executions
        for _ in 0..5 {
            db.save(mock_execution("job-1", true), true).await.unwrap();
        }

        // Cleanup: keep only 2 per job
        db.cleanup(vec!["job-1".into()], 2, 10000).await.unwrap();

        let history = db.list("job-1", 10).await.unwrap();
        assert_eq!(history.len(), 2);
    }
}
