use std::collections::HashSet;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::{bail, Context, Result};
use nix::sys::signal;
use nix::unistd::Pid;
use rusqlite::Connection;
use time::OffsetDateTime;

const SUBNET_PREFIX: &str = "172.16.0";
const IP_RANGE_START: u8 = 2;
const IP_RANGE_END: u8 = 254;

pub fn vms_dir() -> PathBuf {
    PathBuf::from("vume/vms")
}

fn db_path() -> PathBuf {
    vms_dir().join("vume.db")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VmStatus {
    Booting,
    Running,
    Stopped,
    Error,
}

impl VmStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Booting => "booting",
            Self::Running => "running",
            Self::Stopped => "stopped",
            Self::Error => "error",
        }
    }
}

impl FromStr for VmStatus {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s {
            "booting" => Ok(Self::Booting),
            "running" => Ok(Self::Running),
            "stopped" => Ok(Self::Stopped),
            "error" => Ok(Self::Error),
            _ => bail!("Unknown VM status: {s}"),
        }
    }
}

impl fmt::Display for VmStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(self.as_str())
    }
}

#[derive(Debug, Clone)]
pub struct VMInfo {
    pub id: String,
    pub pid: i64,
    pub ip: String,
    pub tap: String,
    pub status: VmStatus,
    pub created_at: String,
}

impl VMInfo {
    /// Return the PID as a u32, or `None` if it is zero or negative.
    pub fn pid_u32(&self) -> Option<u32> {
        u32::try_from(self.pid).ok().filter(|&p| p > 0)
    }

    fn from_row(row: &rusqlite::Row) -> rusqlite::Result<Self> {
        let status_str: String = row.get("status")?;
        let status = status_str.parse::<VmStatus>().map_err(|e| {
            rusqlite::Error::InvalidColumnType(
                0,
                format!("status column: {e}"),
                rusqlite::types::Type::Text,
            )
        })?;
        Ok(Self {
            id: row.get("id")?,
            pid: row.get("pid")?,
            ip: row.get("ip")?,
            tap: row.get("tap")?,
            status,
            created_at: row.get("created_at")?,
        })
    }
}

pub struct StateManager {
    conn: Connection,
}

impl StateManager {
    pub fn new() -> Result<Self> {
        Self::with_path(&db_path())
    }

    pub fn with_path(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let conn = Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA foreign_keys=ON;
             CREATE TABLE IF NOT EXISTS vms (
                 id          TEXT PRIMARY KEY,
                 pid         INTEGER NOT NULL,
                 ip          TEXT NOT NULL,
                 tap         TEXT NOT NULL,
                 status      TEXT NOT NULL DEFAULT 'running',
                 created_at  TEXT NOT NULL
             );
             CREATE UNIQUE INDEX IF NOT EXISTS uq_active_ip
             ON vms (ip) WHERE status IN ('running', 'booting');",
        )?;

        Ok(Self { conn })
    }

    pub fn reserve_vm(&self, vm_id: &str, tap: &str) -> Result<VMInfo> {
        self.exclusive(|conn| {
            let ip = allocate_ip(conn)?;
            let created_at = utc_now()?;
            let mut stmt = conn.prepare(
                "INSERT INTO vms (id, pid, ip, tap, status, created_at)
                 VALUES (?1, 0, ?2, ?3, ?4, ?5) RETURNING *",
            )?;
            let mut rows = stmt.query_map(
                rusqlite::params![vm_id, ip, tap, VmStatus::Booting.as_str(), created_at],
                VMInfo::from_row,
            )?;
            rows.next()
                .context("No row returned from INSERT")?
                .context("Failed to read inserted row")
        })
    }

    pub fn resume_vm(&self, vm_id: &str, tap: &str) -> Result<VMInfo> {
        self.exclusive(|conn| {
            let ip = allocate_ip(conn)?;
            let mut stmt = conn.prepare(
                "UPDATE vms SET pid = 0, ip = ?1, tap = ?2, status = ?3
                 WHERE id = ?4 RETURNING *",
            )?;
            let mut rows = stmt.query_map(
                rusqlite::params![ip, tap, VmStatus::Booting.as_str(), vm_id],
                VMInfo::from_row,
            )?;
            rows.next()
                .context("VM not found")?
                .context("Failed to read updated row")
        })
    }

    pub fn mark_running(&self, vm_id: &str, pid: i64) -> Result<VMInfo> {
        let mut stmt = self
            .conn
            .prepare("UPDATE vms SET status = ?1, pid = ?2 WHERE id = ?3 RETURNING *")?;
        let mut rows = stmt.query_map(
            rusqlite::params![VmStatus::Running.as_str(), pid, vm_id],
            VMInfo::from_row,
        )?;
        rows.next()
            .context("VM not found")?
            .context("Failed to read updated row")
    }

    pub fn get_vm(&self, vm_id: &str) -> Result<Option<VMInfo>> {
        query_vm(&self.conn, vm_id)
    }

    pub fn list_vms(&self, status: Option<VmStatus>) -> Result<Vec<VMInfo>> {
        let (sql, params): (&str, Vec<Box<dyn rusqlite::types::ToSql>>) = match status {
            Some(s) => (
                "SELECT * FROM vms WHERE status = ?1 ORDER BY created_at",
                vec![Box::new(s.as_str().to_owned())],
            ),
            None => ("SELECT * FROM vms ORDER BY created_at", vec![]),
        };
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(params), VMInfo::from_row)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("Failed to read VM rows")
    }

    pub fn update_status(&self, vm_id: &str, status: VmStatus) -> Result<VMInfo> {
        let mut stmt = self
            .conn
            .prepare("UPDATE vms SET status = ?1 WHERE id = ?2 RETURNING *")?;
        let mut rows =
            stmt.query_map(rusqlite::params![status.as_str(), vm_id], VMInfo::from_row)?;
        rows.next()
            .context("VM not found")?
            .context("Failed to read updated row")
    }

    pub fn delete_vm(&self, vm_id: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM vms WHERE id = ?1", rusqlite::params![vm_id])?;
        Ok(())
    }

    pub fn refresh_status(&self) -> Result<Vec<VMInfo>> {
        let mut stale = Vec::new();

        for info in self.list_vms(Some(VmStatus::Running))? {
            if !info.pid_u32().is_some_and(pid_alive) {
                stale.push(self.update_status(&info.id, VmStatus::Error)?);
            }
        }
        for info in self.list_vms(Some(VmStatus::Booting))? {
            stale.push(self.update_status(&info.id, VmStatus::Error)?);
        }

        Ok(stale)
    }

    fn exclusive<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        self.conn.execute_batch("BEGIN EXCLUSIVE")?;
        match f(&self.conn) {
            Ok(val) => {
                self.conn.execute_batch("COMMIT")?;
                Ok(val)
            }
            Err(e) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                Err(e)
            }
        }
    }
}

fn query_vm(conn: &Connection, vm_id: &str) -> Result<Option<VMInfo>> {
    let mut stmt = conn.prepare("SELECT * FROM vms WHERE id = ?1")?;
    let mut rows = stmt.query_map(rusqlite::params![vm_id], VMInfo::from_row)?;
    match rows.next() {
        Some(row) => Ok(Some(row?)),
        None => Ok(None),
    }
}

fn allocate_ip(conn: &Connection) -> Result<String> {
    let mut stmt = conn.prepare("SELECT ip FROM vms WHERE status IN ('running', 'booting')")?;
    let used: HashSet<String> = stmt
        .query_map([], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;

    for i in IP_RANGE_START..=IP_RANGE_END {
        let candidate = format!("{SUBNET_PREFIX}.{i}");
        if !used.contains(&candidate) {
            return Ok(candidate);
        }
    }
    bail!("No available IPs in pool")
}

fn pid_alive(pid: u32) -> bool {
    signal::kill(Pid::from_raw(pid as i32), None).is_ok()
}

fn utc_now() -> Result<String> {
    OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Iso8601::DEFAULT)
        .context("Failed to format timestamp")
}
