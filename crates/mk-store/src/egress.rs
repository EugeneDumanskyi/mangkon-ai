//! The project's egress tables (schema v8, BC-11): endpoint permissions and egress records.
//! Classes and purposes are the stored words of `mk-egress`; this crate does not interpret them.

use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::StoreError;
use crate::time::now_iso;

/// A stored permission (domain §7.6). No row is Not allowed (INV-EG-01).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PermissionRow {
    pub endpoint_id: String,
    /// `scheme://host:port` as confirmed.
    pub confirmed_origin: String,
    /// `local_network` or `internet`.
    pub confirmed_class: String,
    /// `allowed` or `needs_reconfirmation`.
    pub state: String,
    pub reason: Option<String>,
    pub confirmed_at: String,
    pub updated_at: String,
}

/// One destination that received project data (INV-EG-05).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EgressRecordRow {
    pub host: String,
    pub class: String,
    pub purpose: String,
    pub first_sent_at: String,
    pub last_sent_at: String,
}

fn permission_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<PermissionRow> {
    Ok(PermissionRow {
        endpoint_id: r.get(0)?,
        confirmed_origin: r.get(1)?,
        confirmed_class: r.get(2)?,
        state: r.get(3)?,
        reason: r.get(4)?,
        confirmed_at: r.get(5)?,
        updated_at: r.get(6)?,
    })
}

const PERMISSION_COLUMNS: &str =
    "endpoint_id, confirmed_origin, confirmed_class, state, reason, confirmed_at, updated_at";

pub(crate) fn permission(c: &Connection, id: &str) -> Result<Option<PermissionRow>, StoreError> {
    Ok(c.prepare_cached(&format!(
        "SELECT {PERMISSION_COLUMNS} FROM endpoint_permission WHERE endpoint_id = ?1"
    ))?
    .query_row([id], permission_row)
    .optional()?)
}

pub(crate) fn permissions(c: &Connection) -> Result<Vec<PermissionRow>, StoreError> {
    let mut stmt = c.prepare_cached(&format!(
        "SELECT {PERMISSION_COLUMNS} FROM endpoint_permission ORDER BY endpoint_id"
    ))?;
    let rows = stmt.query_map([], permission_row)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Stores a confirmed permission (US-09.7 AC2), replacing any earlier one.
pub(crate) fn allow(
    tx: &Transaction,
    id: &str,
    origin: &str,
    class: &str,
) -> Result<(), StoreError> {
    if !matches!(class, "local_network" | "internet") {
        return Err(StoreError::Invalid(format!(
            "a permission is only stored for a non-local class, not {class}"
        )));
    }
    let now = now_iso();
    tx.prepare_cached(
        "INSERT INTO endpoint_permission
             (endpoint_id, confirmed_origin, confirmed_class, state, reason, confirmed_at, updated_at)
         VALUES (?1, ?2, ?3, 'allowed', NULL, ?4, ?4)
         ON CONFLICT (endpoint_id) DO UPDATE SET
             confirmed_origin = excluded.confirmed_origin,
             confirmed_class = excluded.confirmed_class,
             state = 'allowed', reason = NULL,
             confirmed_at = excluded.confirmed_at, updated_at = excluded.updated_at",
    )?
    .execute(params![id, origin, class, now])?;
    Ok(())
}

/// Moves an allowed permission to "needs reconfirmation" with `reason`. Returns whether a row
/// changed; one already waiting keeps its first reason.
pub(crate) fn needs_reconfirmation(
    tx: &Transaction,
    id: &str,
    reason: &str,
) -> Result<bool, StoreError> {
    Ok(tx
        .prepare_cached(
            "UPDATE endpoint_permission SET state = 'needs_reconfirmation', reason = ?2,
                 updated_at = ?3
             WHERE endpoint_id = ?1 AND state = 'allowed'",
        )?
        .execute(params![id, reason, now_iso()])?
        > 0)
}

/// Revokes: the row goes, so the endpoint is Not allowed again. Returns whether one existed.
pub(crate) fn revoke(tx: &Transaction, id: &str) -> Result<bool, StoreError> {
    Ok(tx
        .prepare_cached("DELETE FROM endpoint_permission WHERE endpoint_id = ?1")?
        .execute([id])?
        > 0)
}

/// Upserts the record before a send (INV-EG-05).
pub(crate) fn record(
    tx: &Transaction,
    host: &str,
    class: &str,
    purpose: &str,
) -> Result<(), StoreError> {
    let now = now_iso();
    tx.prepare_cached(
        "INSERT INTO egress_record (host, class, purpose, first_sent_at, last_sent_at)
         VALUES (?1, ?2, ?3, ?4, ?4)
         ON CONFLICT (host, class, purpose) DO UPDATE SET last_sent_at = excluded.last_sent_at",
    )?
    .execute(params![host, class, purpose, now])?;
    Ok(())
}

/// Every record, the most recent first (US-09.8 AC2).
pub(crate) fn records(c: &Connection) -> Result<Vec<EgressRecordRow>, StoreError> {
    let mut stmt = c.prepare_cached(
        "SELECT host, class, purpose, first_sent_at, last_sent_at FROM egress_record
         ORDER BY last_sent_at DESC, host, class, purpose",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(EgressRecordRow {
            host: r.get(0)?,
            class: r.get(1)?,
            purpose: r.get(2)?,
            first_sent_at: r.get(3)?,
            last_sent_at: r.get(4)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}
