use super::DatabaseManager;
use crate::error::{Error, Result};

impl DatabaseManager {
    #[cfg(feature = "dep-manager")]
    pub async fn get_pending_packages(&self) -> Result<Vec<crate::dep_manager::ManagedPackage>> {
        self.conn
            .call(|conn| -> rusqlite::Result<_> {
                let mut stmt = conn.prepare(
                    "SELECT id, pkg_type, name, status, target, current, error_msg, created_at, updated_at
                 FROM packages
                 WHERE status IN (0, 2)", // Failed(0) or Pending(2)
                )?;
                let rows = stmt.query_map([], Self::map_row_to_package)?;
                rows.collect()
            })
            .await
            .map_err(|e| Error::Config(e.to_string()))
    }

    #[cfg(feature = "dep-manager")]
    pub async fn update_package_status(
        &self,
        id: i64,
        status: crate::dep_manager::PkgStatus,
        error_msg: Option<String>,
        current: Option<String>,
    ) -> Result<()> {
        let status_val = status as i64;
        self.conn.call(move |conn| -> rusqlite::Result<_> {
            conn.execute(
                "UPDATE packages SET status = ?1, error_msg = ?2, current = ?3, updated_at = datetime('now') WHERE id = ?4",
                rusqlite::params![status_val, error_msg, current, id],
            )?;
            Ok(())
        }).await.map_err(|e| Error::Config(e.to_string()))
    }

    #[cfg(feature = "dep-manager")]
    pub async fn add_packages(
        &self,
        pkg_type: crate::dep_manager::PkgType,
        packages: Vec<(String, Option<String>)>,
    ) -> Result<Vec<i64>> {
        let pkg_type_str = pkg_type.to_string();
        self.conn.call(move |conn| -> rusqlite::Result<_> {
            let tx = conn.transaction()?;
            let mut ids = Vec::with_capacity(packages.len());
            for (name, target) in packages {
                tx.execute(
                    "INSERT INTO packages (pkg_type, name, status, target, created_at, updated_at)
                     VALUES (?1, ?2, 2, ?3, datetime('now'), datetime('now'))
                     ON CONFLICT(pkg_type, name) DO UPDATE SET status = 2, target = excluded.target, updated_at = datetime('now')",
                    rusqlite::params![pkg_type_str, name, target],
                )?;
                ids.push(tx.query_row(
                    "SELECT id FROM packages WHERE pkg_type = ?1 AND name = ?2",
                    rusqlite::params![pkg_type_str, name],
                    |row| row.get(0),
                )?);
            }
            tx.commit()?;
            Ok(ids)
        }).await.map_err(|e| Error::Config(e.to_string()))
    }

    #[cfg(feature = "dep-manager")]
    pub async fn delete_packages(&self, ids: Vec<i64>) -> Result<()> {
        self.conn
            .call(move |conn| -> rusqlite::Result<_> {
                let tx = conn.transaction()?;
                for id in ids {
                    if tx.execute("DELETE FROM packages WHERE id = ?1", rusqlite::params![id])? != 1
                    {
                        return Err(rusqlite::Error::QueryReturnedNoRows);
                    }
                }
                tx.commit()?;
                Ok(())
            })
            .await
            .map_err(|e| Error::Config(e.to_string()))
    }

    #[cfg(feature = "dep-manager")]
    pub async fn list_packages(&self) -> Result<Vec<crate::dep_manager::ManagedPackage>> {
        self.conn
            .call(|conn| -> rusqlite::Result<_> {
                let mut stmt = conn.prepare(
                    "SELECT id, pkg_type, name, status, target, current, error_msg, created_at, updated_at
                 FROM packages
                 ORDER BY created_at DESC",
                )?;
                let rows = stmt.query_map([], Self::map_row_to_package)?;
                rows.collect()
            })
            .await
            .map_err(|e| Error::Config(e.to_string()))
    }

    fn map_row_to_package(
        row: &rusqlite::Row,
    ) -> rusqlite::Result<crate::dep_manager::ManagedPackage> {
        Ok(crate::dep_manager::ManagedPackage {
            id: row.get(0)?,
            pkg_type: std::str::FromStr::from_str(&row.get::<_, String>(1)?)
                .unwrap_or(crate::dep_manager::PkgType::APT),
            name: row.get(2)?,
            status: row.get::<_, i64>(3)?.into(),
            target: row.get(4)?,
            current: row.get(5)?,
            error_msg: row.get(6)?,
            created_at: row.get(7)?,
            updated_at: row.get(8)?,
        })
    }
}
