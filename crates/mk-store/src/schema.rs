//! Opening a project database: version check, backup, migrations, derived rebuild (domain §7.9,
//! ADR-0004, INV-WS-03/04, NFR-REL-11).

use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, OptionalExtension};

use crate::StoreError;
use crate::derive::{DERIVED_VERSION, rebuild_all};

/// One forward-only schema step, embedded in the binary (NFR-MNT-07).
#[derive(Clone, Copy, Debug)]
pub struct Migration {
    pub version: u32,
    pub name: &'static str,
    pub sql: &'static str,
}

/// The app's migrations, in order. The last one's version is the app's schema version.
pub const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "init",
        sql: include_str!("../migrations/0001_init.sql"),
    },
    Migration {
        version: 2,
        name: "text_formats",
        sql: include_str!("../migrations/0002_text_formats.sql"),
    },
    Migration {
        version: 3,
        name: "pages",
        sql: include_str!("../migrations/0003_pages.sql"),
    },
    Migration {
        version: 4,
        name: "projects_queue",
        sql: include_str!("../migrations/0004_projects_queue.sql"),
    },
    Migration {
        version: 5,
        name: "search",
        sql: include_str!("../migrations/0005_search.sql"),
    },
    Migration {
        version: 6,
        name: "identifiers",
        sql: include_str!("../migrations/0006_identifiers.sql"),
    },
    Migration {
        version: 7,
        name: "mapping",
        sql: include_str!("../migrations/0007_mapping.sql"),
    },
];

/// What opening did.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct OpenReport {
    /// Schema version found; 0 for a new database.
    pub from_version: u32,
    pub to_version: u32,
    /// The copy written before migrating (INV-WS-04). None for a new or current database.
    pub backup: Option<PathBuf>,
    /// True when the derived tables were rebuilt (after a migration or a derived change).
    pub derived_rebuilt: bool,
}

pub(crate) fn configure(conn: &Connection) -> Result<(), StoreError> {
    conn.busy_timeout(Duration::from_secs(10))?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    Ok(())
}

fn user_version(conn: &Connection) -> Result<u32, StoreError> {
    Ok(conn.pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))?)
}

/// `SQLITE_READONLY_ROLLBACK`: a read-only connection found a hot journal.
fn is_hot_journal(e: &rusqlite::Error) -> bool {
    e.sqlite_error()
        .is_some_and(|e| e.extended_code == rusqlite::ffi::SQLITE_READONLY_ROLLBACK)
}

/// Opens (or creates) the database at `path` for writing, bringing it to the last version in
/// `migrations`. Takes the list as a parameter so tests can add a v2 or a broken one.
///
/// - Newer than `migrations` → [`StoreError::TooNew`]; the file is only read (INV-WS-03).
/// - Older and not empty → `VACUUM INTO <backups>/project-v<old>-<UTC>.db`, then every pending
///   migration and the derived rebuild in **one** transaction. If anything fails, that
///   transaction rolls back: the project stays at the old version and the backup is kept
///   ([`StoreError::MigrationFailed`], INV-WS-04, NFR-REL-11).
pub fn open_write(
    path: &Path,
    backups: &Path,
    migrations: &[Migration],
) -> Result<(Connection, OpenReport), StoreError> {
    let latest = migrations.last().map_or(0, |m| m.version);
    if path.exists() {
        let ro = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let found = match user_version(&ro) {
            // A kill during a write before WAL is on (while the file is created) leaves a hot
            // rollback journal, which a read-only connection cannot roll back. A read-write
            // connection rolls it back on its first read, back to the last commit (NFR-REL-03).
            Err(StoreError::Sqlite(e)) if is_hot_journal(&e) => {
                drop(ro);
                user_version(&Connection::open(path)?)?
            }
            r => r?,
        };
        if found > latest {
            return Err(StoreError::TooNew {
                found,
                supported: latest,
                app_version: stored_app_version(path),
            });
        }
    }
    let mut conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    configure(&conn)?;
    // 64 MiB of page cache for the one writer. The identifier-key index is filled in random key
    // order, so the default 2 MiB made a 500 MB CSV commit (10.7M mentions) take 65 s instead
    // of 54 s (NFR-CAP-05). Readers keep the default; `shrink_memory` frees it when idle.
    conn.pragma_update(None, "cache_size", -65536)?;
    let from = user_version(&conn)?;

    let mut report = OpenReport {
        from_version: from,
        to_version: from,
        backup: None,
        derived_rebuilt: false,
    };
    if from < latest {
        if from > 0 {
            report.backup = Some(backup(&conn, backups, from)?);
        }
        let failed = |reason: String| StoreError::MigrationFailed {
            from,
            to: latest,
            backup: report.backup.clone(),
            reason,
        };
        let tx = conn.transaction().map_err(|e| failed(e.to_string()))?;
        for m in migrations.iter().filter(|m| m.version > from) {
            tx.execute_batch(m.sql)
                .map_err(|e| failed(format!("migration {} ({}): {e}", m.version, m.name)))?;
        }
        tx.pragma_update(None, "user_version", latest)
            .map_err(|e| failed(e.to_string()))?;
        rebuild_all(&tx).map_err(|e| failed(format!("derived rebuild: {e}")))?;
        tx.commit().map_err(|e| failed(e.to_string()))?;
        report.to_version = latest;
        report.derived_rebuilt = true;
    } else {
        let derived: Option<String> = conn
            .query_row(
                "SELECT value FROM project_meta WHERE key = 'derived_version'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        if derived.as_deref() != Some(DERIVED_VERSION.to_string().as_str()) {
            let tx = conn.transaction()?;
            rebuild_all(&tx)?;
            tx.commit()?;
            report.derived_rebuilt = true;
        }
    }
    Ok((conn, report))
}

/// The app version a project was made or last migrated by, if it says (from schema v4). Read
/// from a too-new project without changing it (US-01.2 AC3).
fn stored_app_version(path: &Path) -> Option<String> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
    conn.query_row(
        "SELECT value FROM project_meta WHERE key = 'app_version'",
        [],
        |r| r.get(0),
    )
    .ok()
}

/// A read connection: `query_only`, so it can never write.
pub(crate) fn open_read(path: &Path) -> Result<Connection, StoreError> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    configure(&conn)?;
    conn.pragma_update(None, "query_only", "ON")?;
    Ok(conn)
}

/// `VACUUM INTO` a fresh file in `dir`: a consistent copy, even with WAL (INV-WS-04).
fn backup(conn: &Connection, dir: &Path, version: u32) -> Result<PathBuf, StoreError> {
    std::fs::create_dir_all(dir)?;
    let stamp = crate::time::now_compact();
    let mut target = dir.join(format!("project-v{version}-{stamp}.db"));
    let mut n = 2;
    while target.exists() {
        target = dir.join(format!("project-v{version}-{stamp}-{n}.db"));
        n += 1;
    }
    let as_str = target
        .to_str()
        .ok_or_else(|| StoreError::Invalid("backup path is not valid Unicode".into()))?;
    conn.execute("VACUUM INTO ?1", [as_str])?;
    Ok(target)
}
