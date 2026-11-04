//! `app.db` (architecture §6, ADR-0004): the app's own database, next to the projects. M3c: the
//! project registry (domain §5.2, US-01.1, US-01.2). M5a: external model endpoints and their
//! tests (US-09.6); their API keys are in the OS secret store, never here (INV-EG-07). M5b:
//! model downloads and verified files (domain §7.7), app settings (the Q7 override). Few,
//! small writes from one thread at a time: a plain connection behind a mutex, no writer thread.
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
pub const APP_MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "registry",
        sql: include_str!("../app_migrations/0001_registry.sql"),
    },
    Migration {
        version: 2,
        name: "endpoints",
        sql: include_str!("../app_migrations/0002_endpoints.sql"),
    },
    Migration {
        version: 3,
        name: "models",
        sql: include_str!("../app_migrations/0003_models.sql"),
    },
];

/// A model download that left "Available" (domain §5.3 MODEL_INSTALLATION). `state` is a
/// §7.7 state name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelDownloadRow {
    pub model_id: String,
    pub state: String,
    pub bytes_done: i64,
    pub total_bytes: i64,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub updated_at: i64,
}

/// Tests kept per endpoint; older ones are dropped.
const KEPT_TESTS: i64 = 20;

/// An external model endpoint (domain §5.2 ENDPOINT). No key: `secret_ref` names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EndpointRow {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub model_name: String,
    pub secret_ref: Option<String>,
    /// `None` until tested (US-09.6 AC3).
    pub structured_output: Option<bool>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// One endpoint test (domain §5.2 ENDPOINT_TEST).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EndpointTestRow {
    pub endpoint_id: String,
    pub tested_at: i64,
    pub ok: bool,
    pub message: String,
    pub response_ms: Option<i64>,
    pub reported_model: Option<String>,
}

const ENDPOINT_COLUMNS: &str =
    "id, name, base_url, model_name, secret_ref, structured_output, created_at, updated_at";

fn endpoint_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<EndpointRow> {
    Ok(EndpointRow {
        id: r.get(0)?,
        name: r.get(1)?,
        base_url: r.get(2)?,
        model_name: r.get(3)?,
        secret_ref: r.get(4)?,
        structured_output: r.get::<_, Option<i64>>(5)?.map(|v| v != 0),
        created_at: r.get(6)?,
        updated_at: r.get(7)?,
    })
}

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
        // Domain §7.7: a download or check that was running when the app quit is Interrupted.
        // (Tests open older schemas, without the table.)
        if latest >= 3 {
            conn.execute(
                "UPDATE model_download SET state = 'interrupted', updated_at = ?1
                 WHERE state IN ('downloading', 'verifying')",
                [now_ms()],
            )?;
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

    // ---- endpoints (M5a) ----

    /// Every endpoint, by name.
    pub fn endpoints(&self) -> Result<Vec<EndpointRow>, StoreError> {
        let conn = self.lock();
        let mut stmt = conn.prepare_cached(&format!(
            "SELECT {ENDPOINT_COLUMNS} FROM endpoint ORDER BY name COLLATE NOCASE, id"
        ))?;
        let rows = stmt.query_map([], endpoint_row)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn endpoint(&self, id: &str) -> Result<Option<EndpointRow>, StoreError> {
        let conn = self.lock();
        Ok(conn
            .prepare_cached(&format!(
                "SELECT {ENDPOINT_COLUMNS} FROM endpoint WHERE id = ?1"
            ))?
            .query_row([id], endpoint_row)
            .optional()?)
    }

    pub fn insert_endpoint(&self, e: &EndpointRow) -> Result<(), StoreError> {
        self.lock()
            .prepare_cached(&format!(
                "INSERT INTO endpoint ({ENDPOINT_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)"
            ))?
            .execute(params![
                e.id,
                e.name,
                e.base_url,
                e.model_name,
                e.secret_ref,
                e.structured_output.map(i64::from),
                e.created_at,
                e.updated_at
            ])?;
        Ok(())
    }

    /// Updates an endpoint. A new URL or model forgets the structured-output result, which
    /// was found for the old one. Returns whether the endpoint exists.
    pub fn update_endpoint(
        &self,
        id: &str,
        name: &str,
        base_url: &str,
        model_name: &str,
        secret_ref: Option<&str>,
    ) -> Result<bool, StoreError> {
        Ok(self
            .lock()
            .prepare_cached(
                "UPDATE endpoint SET name = ?2, base_url = ?3, model_name = ?4, secret_ref = ?5,
                     structured_output = CASE WHEN base_url = ?3 AND model_name = ?4
                         THEN structured_output END,
                     updated_at = ?6
                 WHERE id = ?1",
            )?
            .execute(params![
                id,
                name,
                base_url,
                model_name,
                secret_ref,
                now_ms()
            ])?
            > 0)
    }

    /// Deletes an endpoint and its tests. Returns whether it existed.
    pub fn delete_endpoint(&self, id: &str) -> Result<bool, StoreError> {
        Ok(self
            .lock()
            .prepare_cached("DELETE FROM endpoint WHERE id = ?1")?
            .execute([id])?
            > 0)
    }

    /// Stores a test and, when the test got that far, whether structured output works.
    pub fn record_endpoint_test(
        &self,
        t: &EndpointTestRow,
        structured_output: Option<bool>,
    ) -> Result<(), StoreError> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        tx.prepare_cached(
            "INSERT INTO endpoint_test
                 (endpoint_id, tested_at, ok, message, response_ms, reported_model)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )?
        .execute(params![
            t.endpoint_id,
            t.tested_at,
            t.ok,
            t.message,
            t.response_ms,
            t.reported_model
        ])?;
        tx.prepare_cached(
            "DELETE FROM endpoint_test WHERE endpoint_id = ?1 AND id NOT IN
                 (SELECT id FROM endpoint_test WHERE endpoint_id = ?1
                  ORDER BY tested_at DESC, id DESC LIMIT ?2)",
        )?
        .execute(params![t.endpoint_id, KEPT_TESTS])?;
        if let Some(s) = structured_output {
            tx.prepare_cached("UPDATE endpoint SET structured_output = ?2 WHERE id = ?1")?
                .execute(params![t.endpoint_id, i64::from(s)])?;
        }
        tx.commit()?;
        Ok(())
    }

    /// The newest test of an endpoint.
    pub fn last_endpoint_test(&self, id: &str) -> Result<Option<EndpointTestRow>, StoreError> {
        let conn = self.lock();
        Ok(conn
            .prepare_cached(
                "SELECT endpoint_id, tested_at, ok, message, response_ms, reported_model
                 FROM endpoint_test WHERE endpoint_id = ?1
                 ORDER BY tested_at DESC, id DESC LIMIT 1",
            )?
            .query_row([id], |r| {
                Ok(EndpointTestRow {
                    endpoint_id: r.get(0)?,
                    tested_at: r.get(1)?,
                    ok: r.get(2)?,
                    message: r.get(3)?,
                    response_ms: r.get(4)?,
                    reported_model: r.get(5)?,
                })
            })
            .optional()?)
    }

    // ---- model downloads (M5b) ----

    /// Every model download that left "Available".
    pub fn model_downloads(&self) -> Result<Vec<ModelDownloadRow>, StoreError> {
        let conn = self.lock();
        let mut stmt = conn.prepare_cached(
            "SELECT model_id, state, bytes_done, total_bytes, error_code, error_message, updated_at
             FROM model_download ORDER BY model_id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(ModelDownloadRow {
                model_id: r.get(0)?,
                state: r.get(1)?,
                bytes_done: r.get(2)?,
                total_bytes: r.get(3)?,
                error_code: r.get(4)?,
                error_message: r.get(5)?,
                updated_at: r.get(6)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Stores a download's state; `available` removes the row and its verified files.
    pub fn set_model_download(
        &self,
        model_id: &str,
        state: &str,
        bytes_done: u64,
        total_bytes: u64,
        error: Option<(&str, &str)>,
    ) -> Result<(), StoreError> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        if state == "available" {
            tx.prepare_cached("DELETE FROM model_download WHERE model_id = ?1")?
                .execute([model_id])?;
            tx.prepare_cached("DELETE FROM model_file WHERE model_id = ?1")?
                .execute([model_id])?;
        } else {
            tx.prepare_cached(
                "INSERT INTO model_download
                     (model_id, state, bytes_done, total_bytes, error_code, error_message,
                      updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT (model_id) DO UPDATE SET state = ?2, bytes_done = ?3,
                     total_bytes = ?4, error_code = ?5, error_message = ?6, updated_at = ?7",
            )?
            .execute(params![
                model_id,
                state,
                bytes_done as i64,
                total_bytes as i64,
                error.map(|e| e.0),
                error.map(|e| e.1),
                now_ms()
            ])?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Updates only the byte count of a running download.
    pub fn set_model_bytes(&self, model_id: &str, bytes_done: u64) -> Result<(), StoreError> {
        self.lock()
            .prepare_cached(
                "UPDATE model_download SET bytes_done = ?2, updated_at = ?3 WHERE model_id = ?1",
            )?
            .execute(params![model_id, bytes_done as i64, now_ms()])?;
        Ok(())
    }

    /// Records a file whose checksum matched the catalog (INV-MM-01).
    pub fn model_file_verified(
        &self,
        model_id: &str,
        name: &str,
        bytes: u64,
    ) -> Result<(), StoreError> {
        self.lock()
            .prepare_cached(
                "INSERT OR REPLACE INTO model_file (model_id, name, bytes, verified_at)
                 VALUES (?1, ?2, ?3, ?4)",
            )?
            .execute(params![model_id, name, bytes as i64, now_ms()])?;
        Ok(())
    }

    /// The files of a model recorded as verified, with their sizes.
    pub fn model_files_verified(&self, model_id: &str) -> Result<Vec<(String, u64)>, StoreError> {
        let conn = self.lock();
        let mut stmt = conn.prepare_cached(
            "SELECT name, bytes FROM model_file WHERE model_id = ?1 ORDER BY name",
        )?;
        let rows = stmt.query_map([model_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as u64))
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    // ---- app settings (M5b) ----

    pub fn setting(&self, key: &str) -> Result<Option<String>, StoreError> {
        Ok(self
            .lock()
            .prepare_cached("SELECT value FROM app_setting WHERE key = ?1")?
            .query_row([key], |r| r.get(0))
            .optional()?)
    }

    /// Sets a setting; `None` removes it.
    pub fn set_setting(&self, key: &str, value: Option<&str>) -> Result<(), StoreError> {
        let conn = self.lock();
        match value {
            Some(v) => conn
                .prepare_cached("INSERT OR REPLACE INTO app_setting (key, value) VALUES (?1, ?2)")?
                .execute(params![key, v])?,
            None => conn
                .prepare_cached("DELETE FROM app_setting WHERE key = ?1")?
                .execute([key])?,
        };
        Ok(())
    }
}
