//! Step-debug (ADR 0013): the debug router's contract and its conformance check, and the store of
//! finished runs in the target project (CON-24, INV-13).

use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The contract every debug router is built by (CON-27).
pub const CONTRACT: &str = include_str!("../contract/debug-router.openapi.yaml");

/// One `flowstate_update` of a run, off-chart ones too (INV-13).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Step {
    pub node: u64,
    pub state: String,
    pub off_chart: bool,
    pub at_ms: u64,
    pub data: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Run {
    pub id: i64,
    pub map: u64,
    pub instance: String,
    pub steps: Vec<Step>,
}

fn store_path(root: &str) -> PathBuf {
    Path::new(root).join(".harness").join("debug").join("runs.sqlite")
}

fn open(path: &Path) -> Result<Connection, String> {
    let conn = Connection::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    conn.execute_batch("CREATE TABLE IF NOT EXISTS runs (id INTEGER PRIMARY KEY, map INTEGER NOT NULL, instance TEXT NOT NULL, steps TEXT NOT NULL)").map_err(|e| e.to_string())?;
    Ok(conn)
}

/// Saves a finished run, creating the store if this is the project's first (CON-24).
pub fn save_run(root: &str, map: u64, instance: &str, steps: &[Step]) -> Result<i64, String> {
    let path = store_path(root);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let conn = open(&path)?;
    let steps = serde_json::to_string(steps).map_err(|e| e.to_string())?;
    conn.execute("INSERT INTO runs (map, instance, steps) VALUES (?1, ?2, ?3)", params![map as i64, instance, steps]).map_err(|e| e.to_string())?;
    Ok(conn.last_insert_rowid())
}

/// The project's latest runs, newest first. No store yet means no runs, and creates nothing.
pub fn runs(root: &str, limit: usize) -> Result<Vec<Run>, String> {
    let path = store_path(root);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let conn = open(&path)?;
    let mut statement = conn.prepare("SELECT id, map, instance, steps FROM runs ORDER BY id DESC LIMIT ?1").map_err(|e| e.to_string())?;
    let rows = statement.query_map(params![limit as i64], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?, row.get::<_, String>(2)?, row.get::<_, String>(3)?))).map_err(|e| e.to_string())?;
    let mut found = Vec::new();
    for row in rows {
        let (id, map, instance, steps) = row.map_err(|e| e.to_string())?;
        found.push(Run { id, map: map as u64, instance, steps: serde_json::from_str(&steps).map_err(|e| e.to_string())? });
    }
    Ok(found)
}

/// One line of the conformance report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub ok: bool,
    pub line: String,
}

/// Tests a running router against the contract. It does not approve anything unless `approve`
/// is set, since that would move the pipeline on; a step back is only tried without a token.
pub fn check_router(base: &str, token: Option<&str>, approve: bool) -> Vec<Check> {
    let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).timeout_global(Some(Duration::from_secs(10))).build().into();
    let base = base.trim_end_matches('/');
    let status = |method: &str, path: &str, with_token: bool| -> Result<u16, String> {
        let url = format!("{base}{path}");
        let bearer = token.filter(|_| with_token).map(|t| format!("Bearer {t}"));
        let sent = if method == "GET" {
            let mut request = agent.get(&url);
            if let Some(b) = &bearer {
                request = request.header("authorization", b);
            }
            request.call()
        } else {
            let mut request = agent.post(&url);
            if let Some(b) = &bearer {
                request = request.header("authorization", b);
            }
            request.send("{}")
        };
        sent.map(|r| r.status().as_u16()).map_err(|e| format!("{e}"))
    };
    let mut report = Vec::new();
    let mut note = |ok: bool, line: String| report.push(Check { ok, line });

    for (method, path) in [("GET", "/debug/status"), ("POST", "/debug/approve")] {
        match status(method, path, false) {
            Ok(401 | 403) => note(true, format!("{method} {path}: refuses a request with no token")),
            Ok(404) => note(false, format!("{method} {path}: missing, and the contract requires it")),
            Ok(code) => note(false, format!("{method} {path}: answered {code} with no token (CON-25 requires authorization)")),
            Err(e) => note(false, format!("{method} {path}: {e}")),
        }
    }
    match status("POST", "/debug/step-back", false) {
        Ok(401 | 403) => note(true, "POST /debug/step-back: present, refuses a request with no token".to_string()),
        Ok(404) => note(true, "POST /debug/step-back: absent, as it must be where a step back is not possible".to_string()),
        Ok(501) => note(false, "POST /debug/step-back: a stub (501); CON-27 says leave the route out".to_string()),
        Ok(code) => note(false, format!("POST /debug/step-back: answered {code} with no token")),
        Err(e) => note(false, format!("POST /debug/step-back: {e}")),
    }
    let Some(_) = token else {
        note(true, "no MULTITOOL_DEBUG_TOKEN given: routes were not tried with a token".to_string());
        return report;
    };
    match status("GET", "/debug/status", true) {
        Ok(200) => note(true, "GET /debug/status: answers with the token".to_string()),
        Ok(code) => note(false, format!("GET /debug/status: answered {code} with the token")),
        Err(e) => note(false, format!("GET /debug/status: {e}")),
    }
    if approve {
        match status("POST", "/debug/approve", true) {
            Ok(200 | 409) => note(true, "POST /debug/approve: answers with the token".to_string()),
            Ok(code) => note(false, format!("POST /debug/approve: answered {code} with the token")),
            Err(e) => note(false, format!("POST /debug/approve: {e}")),
        }
    }
    report
}
