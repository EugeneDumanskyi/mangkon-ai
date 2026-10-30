//! `app.db` (architecture §6, ADR-0004): the app's own database, next to the projects. M3c: the
//! project registry (domain §5.2, US-01.1, US-01.2). Few, small writes from one thread at a
//! time: a plain connection behind a mutex, no writer thread.
//!
//! Its schema has its own migrations ([`APP_MIGRATIONS`]); a newer one is refused unchanged and
//! an older one is backed up before it migrates, as for `project.db` (INV-WS-03/04).

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use mk_workspace::{ProjectName, RegistryEntry, newest_first};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};

use crate::StoreError;
use crate::schema::{Migration, configure};
use crate::time::{now_compact, now_ms};

/// The app database's migrations, in order.
pub const APP_MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    name: "registry",
    sql: include_str!("../app_migrations/0001_registry.sql"),
}];

/// The app database, `app.db` in the app's data folder.
pub struct AppStore {
    conn: Mutex<Connection>,
    path: PathBuf,
}

fn version(conn: &Connection) -> Result<u32, StoreError> {
    Ok(conn.pragma_query_value(None, "user_version", |r| r.get(0))?)
}

impl AppStore {
    /// Opens or creates `<dir>/app.db` at the app's schema.
    pub fn open(dir: &Path) -> Result<AppStore, StoreError> {
        Self::open_with(dir, APP_MIGRATIONS)
    }

    /// [`open`](Self::open) with another migration list (tests).
    pub fn open_with(dir: &Path, migrations: &[Migration]) -> Result<AppStore, StoreError> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join("app.db");
        let latest = migrations.last().map_or(0, |m| m.version);
        if path.exists() {
            let found = version(&Connection::open_with_flags(
                &path,
                OpenFlags::SQLITE_OPEN_READ_ONLY,
            )?)?;
            if found > latest {
                return Err(StoreError::TooNew {
                    found,
                    supported: latest,
                    app_version: None,
                });
            }
        }
        let mut conn = Connection::open(&path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        configure(&conn)?;
        let from = version(&conn)?;
        if from < latest {
            let backup = if from > 0 {
                let backups = dir.join("backups");
                std::fs::create_dir_all(&backups)?;
                let target = backups.join(format!("app-v{from}-{}.db", now_compact()));
                let s = target.to_str().ok_or_else(|| {
                    StoreError::Invalid("backup path is not valid Unicode".into())
                })?;
                conn.execute("VACUUM INTO ?1", [s])?;
                Some(target)
            } else {
                None
            };
            let failed = |reason: String| StoreError::MigrationFailed {
                from,
                to: latest,
                backup: backup.clone(),
                reason,
            };
            let tx = conn.transaction().map_err(|e| failed(e.to_string()))?;
            for m in migrations.iter().filter(|m| m.version > from) {
                tx.execute_batch(m.sql)
                    .map_err(|e| failed(format!("migration {} ({}): {e}", m.version, m.name)))?;
            }
            tx.pragma_update(None, "user_version", latest)
                .map_err(|e| failed(e.to_string()))?;
            tx.commit().map_err(|e| failed(e.to_string()))?;
        }
        Ok(AppStore {
            conn: Mutex::new(conn),
            path,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn lock(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Every registered project, newest first (US-01.2 AC1).
    pub fn recent(&self) -> Result<Vec<RegistryEntry>, StoreError> {
        let conn = self.lock();
        let mut stmt =
            conn.prepare_cached("SELECT id, name, path, last_opened_at FROM registry_entry")?;
        let rows = stmt.query_map([], |r| {
            Ok(RegistryEntry {
                id: r.get(0)?,
                name: r.get(1)?,
                path: r.get(2)?,
                last_opened_at: r.get(3)?,
            })
        })?;
        let mut out: Vec<RegistryEntry> = rows.collect::<Result<_, _>>()?;
        newest_first(&mut out);
        Ok(out)
    }

    pub fn entry(&self, id: i64) -> Result<Option<RegistryEntry>, StoreError> {
        self.find("id = ?1", params![id])
    }

    pub fn entry_at(&self, path: &str) -> Result<Option<RegistryEntry>, StoreError> {
        self.find("path = ?1", params![path])
    }

    fn find(
        &self,
        clause: &str,
        args: impl rusqlite::Params,
    ) -> Result<Option<RegistryEntry>, StoreError> {
        let conn = self.lock();
        Ok(conn
            .prepare_cached(&format!(
                "SELECT id, name, path, last_opened_at FROM registry_entry WHERE {clause}"
            ))?
            .query_row(args, |r| {
                Ok(RegistryEntry {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    path: r.get(2)?,
                    last_opened_at: r.get(3)?,
                })
            })
            .optional()?)
    }

    /// Registers the project at `path` as opened now. A path already registered keeps its
    /// entry and name. Another entry with the name, ignoring case, is refused
    /// ([`StoreError::NameTaken`], INV-WS-01): the database enforces it on `name_key`.
    pub fn register(&self, name: &ProjectName, path: &str) -> Result<RegistryEntry, StoreError> {
        if let Some(e) = self.entry_at(path)? {
            self.touch(e.id)?;
            return self.entry(e.id)?.ok_or(StoreError::WriterGone);
        }
        {
            let conn = self.lock();
            let taken: Option<i64> = conn
                .prepare_cached("SELECT id FROM registry_entry WHERE name_key = ?1")?
                .query_row([name.key()], |r| r.get(0))
                .optional()?;
            if taken.is_some() {
                return Err(StoreError::NameTaken(name.to_string()));
            }
            conn.prepare_cached(
                "INSERT INTO registry_entry (name, name_key, path, last_opened_at)
                 VALUES (?1, ?2, ?3, ?4)",
            )?
            .execute(params![name.as_str(), name.key(), path, now_ms()])?;
        }
        self.entry_at(path)?.ok_or(StoreError::WriterGone)
    }

    /// Whether another entry has this name, ignoring case (US-01.1 AC2).
    pub fn name_taken(&self, name: &ProjectName) -> Result<bool, StoreError> {
        let conn = self.lock();
        Ok(conn
            .prepare_cached("SELECT EXISTS (SELECT 1 FROM registry_entry WHERE name_key = ?1)")?
            .query_row([name.key()], |r| r.get(0))?)
    }

    /// Marks an entry opened now.
    pub fn touch(&self, id: i64) -> Result<(), StoreError> {
        self.lock()
            .prepare_cached("UPDATE registry_entry SET last_opened_at = ?2 WHERE id = ?1")?
            .execute(params![id, now_ms()])?;
        Ok(())
    }

    /// Points an entry at the folder the project was moved to ("Locate…", US-01.2 AC2).
    pub fn relocate(&self, id: i64, path: &str) -> Result<(), StoreError> {
        let conn = self.lock();
        let other: Option<i64> = conn
            .prepare_cached("SELECT id FROM registry_entry WHERE path = ?1 AND id != ?2")?
            .query_row(params![path, id], |r| r.get(0))
            .optional()?;
        if let Some(other) = other {
            // The new place is registered already: that entry goes, this one moves there.
            conn.prepare_cached("DELETE FROM registry_entry WHERE id = ?1")?
                .execute([other])?;
        }
        conn.prepare_cached(
            "UPDATE registry_entry SET path = ?2, last_opened_at = ?3 WHERE id = ?1",
        )?
        .execute(params![id, path, now_ms()])?;
        Ok(())
    }

    /// Removes an entry from the list ("Remove from list", US-01.2 AC2). The project folder is
    /// not touched.
    pub fn remove(&self, id: i64) -> Result<(), StoreError> {
        self.lock()
            .prepare_cached("DELETE FROM registry_entry WHERE id = ?1")?
            .execute([id])?;
        Ok(())
    }
}
