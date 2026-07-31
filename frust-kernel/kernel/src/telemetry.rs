//! WO-010: observability (REQ-6.4). Structured JSON-lines logs with
//! end-to-end trace IDs, an in-process metrics registry rendered as
//! Prometheus text, and per-tenant attribution â€” all in the kernel's own
//! output. No collector, no sidecar, no new crates: the two-process
//! deployment stays two processes (P-2.3).
//!
//! Trace context is THREAD-LOCAL: the kernel is sync and each request/job
//! runs to completion on one thread (tiny_http accept loop, worker tick),
//! so a thread-local is the whole propagation story. The REST edge mints or
//! adopts a trace; `enqueue` stamps it into the job record; the worker
//! adopts the job's stored trace at run â€” that is how one trace crosses the
//! async boundary. (If the kernel ever goes task-parallel, this becomes a
//! task-local; the emit surface stays the same.)

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

// â”€â”€ trace context â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceId(String);

impl TraceId {
    /// Mints a process-unique id: epoch-nanos xor pid, plus a counter.
    /// Correlation-unique, not cryptographic â€” trace ids are not secrets.
    pub fn new() -> Self {
        static CTR: AtomicU64 = AtomicU64::new(0);
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
        let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let n = CTR.fetch_add(1, Ordering::Relaxed);
        Self(format!(
            "{:08x}{:06x}{:04x}",
            (secs as u32) ^ u32::from(std::process::id() as u16),
            nanos >> 8,
            (n & 0xffff) as u16
        ))
    }

    /// Adopts an externally-supplied id (REST header, job record). Length-
    /// bounded and charset-restricted so log lines stay clean.
    pub fn parse(s: &str) -> Option<Self> {
        let ok = !s.is_empty() && s.len() <= 64 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
        ok.then(|| Self(s.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for TraceId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Default, Clone)]
struct Ctx {
    trace: Option<TraceId>,
    tenant: Option<String>,
}

thread_local! {
    static CTX: RefCell<Ctx> = RefCell::new(Ctx::default());
}

/// RAII scope: installs (trace, tenant) as the thread's current context and
/// restores the previous one on drop.
pub struct CtxGuard(Ctx);

impl Drop for CtxGuard {
    fn drop(&mut self) {
        CTX.with(|c| *c.borrow_mut() = self.0.clone());
    }
}

pub fn enter(trace: TraceId, tenant: &str) -> CtxGuard {
    CTX.with(|c| {
        let prev = c.borrow().clone();
        *c.borrow_mut() = Ctx { trace: Some(trace), tenant: Some(tenant.to_string()) };
        CtxGuard(prev)
    })
}

/// The current trace, minting one if the thread has none (a direct broker
/// call outside REST still gets its children correlated).
pub fn current_trace() -> TraceId {
    CTX.with(|c| {
        let mut ctx = c.borrow_mut();
        ctx.trace.get_or_insert_with(TraceId::new).clone()
    })
}

pub fn current_tenant() -> String {
    CTX.with(|c| c.borrow().tenant.clone().unwrap_or_else(|| "-".into()))
}

/// Sets the tenant for the current scope if none is set (broker verbs know
/// their database; REST already set it).
pub fn enter_tenant(tenant: &str) -> CtxGuard {
    CTX.with(|c| {
        let prev = c.borrow().clone();
        let mut next = prev.clone();
        if next.tenant.is_none() {
            next.tenant = Some(tenant.to_string());
        }
        *c.borrow_mut() = next;
        CtxGuard(prev)
    })
}

// â”€â”€ structured emission â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

/// Log level. Default posture is `Info` â€” enough to answer an incident.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Debug = 0,
    Info = 1,
    Error = 2,
    Off = 3,
}

/// Read live (not cached): tests and operators can flip `FRUST_LOG` at
/// runtime; the env read is ~100 ns, dwarfed by any emission it gates.
fn min_level() -> Level {
    match std::env::var("FRUST_LOG").as_deref() {
        Ok("debug") => Level::Debug,
        Ok("error") => Level::Error,
        Ok("off") => Level::Off,
        _ => Level::Info,
    }
}

/// Recent lines, kept in-process for the reconstruction proof and future
/// debug surfaces. Bounded; oldest lines fall off.
const RING_CAP: usize = 4096;

fn ring() -> &'static Mutex<VecDeque<serde_json::Value>> {
    static RING: OnceLock<Mutex<VecDeque<serde_json::Value>>> = OnceLock::new();
    RING.get_or_init(|| Mutex::new(VecDeque::with_capacity(RING_CAP)))
}

/// Snapshot of recent structured lines (newest last).
pub fn recent() -> Vec<serde_json::Value> {
    ring().lock().unwrap().iter().cloned().collect()
}

/// Emit one structured line: `{ts, lvl, evt, trace, tenant, ...fields}`.
pub fn emit(level: Level, event: &str, fields: &[(&str, serde_json::Value)]) {
    if level < min_level() {
        return;
    }
    let ts = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    let mut obj = serde_json::Map::new();
    obj.insert("ts".into(), serde_json::json!(ts as u64));
    obj.insert(
        "lvl".into(),
        serde_json::json!(match level {
            Level::Debug => "debug",
            Level::Info => "info",
            Level::Error => "error",
            Level::Off => unreachable!(),
        }),
    );
    obj.insert("evt".into(), serde_json::json!(event));
    obj.insert("trace".into(), serde_json::json!(current_trace().as_str()));
    obj.insert("tenant".into(), serde_json::json!(current_tenant()));
    for (k, v) in fields {
        obj.insert((*k).to_string(), v.clone());
    }
    let line = serde_json::Value::Object(obj);
    {
        let mut r = ring().lock().unwrap();
        if r.len() == RING_CAP {
            r.pop_front();
        }
        r.push_back(line.clone());
    }
    // JSON lines on stdout: the kernel's one log stream
    use std::io::Write as _;
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}");
}

/// Extracts the machine code from an error's display form, so `FRUST:E_*`
/// and friends stay grep-able fields (REQ-6.4 criterion 4: codes survive).
pub fn error_code(err: &crate::contract::BrokerError) -> String {
    use crate::contract::BrokerError as E;
    let detail = format!("{err:?}");
    if let Some(at) = detail.find("FRUST:E_") {
        let code: String = detail[at..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == ':' || *c == '_')
            .collect();
        return code;
    }
    match err {
        E::UnknownDoctype { .. } => "E_UNKNOWN_DOCTYPE",
        E::FieldNotReadable { .. } => "E_FIELD_NOT_READABLE",
        E::PathTooDeep { .. } => "E_PATH_TOO_DEEP",
        E::InvalidValue { .. } => "E_INVALID_VALUE",
        E::HookCycle { .. } => "E_HOOK_CYCLE",
        E::HookDepthExceeded { .. } => "E_HOOK_DEPTH",
        E::HookRejected { .. } => "E_HOOK_REJECTED",
        E::PermissionDenied { .. } => "E_PERMISSION_DENIED",
        E::IdentityUnresolved => "E_IDENTITY_UNRESOLVED",
        E::TenantThrottled { .. } => "E_TENANT_THROTTLED",
        // The workflow's own code travels, so a trace names WHICH rule
        // refused rather than merely that one did. Matched to statics, never
        // leaked: this returns `&'static str`, and `Box::leak`ing a fresh
        // string per refusal would make every denied transition a small
        // permanent allocation.
        E::WorkflowDenied { code, .. } => match code.as_str() {
            "FRUST:E_WORKFLOW:UNKNOWN_STATE" => "E_WORKFLOW_UNKNOWN_STATE",
            "FRUST:E_WORKFLOW:UNKNOWN_ACTION" => "E_WORKFLOW_UNKNOWN_ACTION",
            "FRUST:E_WORKFLOW:WRONG_STATE" => "E_WORKFLOW_WRONG_STATE",
            "FRUST:E_WORKFLOW:ROLE_DENIED" => "E_WORKFLOW_ROLE_DENIED",
            "FRUST:E_WORKFLOW:UNMANAGED" => "E_WORKFLOW_UNMANAGED",
            _ => "E_WORKFLOW",
        },
        E::WriteConflictExhausted { .. } => "E_WRITE_CONFLICT_EXHAUSTED",
        E::Db { .. } => "E_DB",
    }
    .to_string()
}

/// Times a span; emits on completion with `dur_us` and outcome fields.
pub struct Span {
    event: &'static str,
    start: Instant,
    fields: Vec<(&'static str, serde_json::Value)>,
}

impl Span {
    pub fn begin(event: &'static str) -> Self {
        Self { event, start: Instant::now(), fields: Vec::new() }
    }

    pub fn field(mut self, k: &'static str, v: impl Into<serde_json::Value>) -> Self {
        self.fields.push((k, v.into()));
        self
    }

    pub fn ok(self) {
        self.finish(Level::Info, None);
    }

    /// Completes successfully at a chosen level (hot paths emit at Debug;
    /// failures always surface via `err`).
    pub fn ok_at(self, level: Level) {
        self.finish(level, None);
    }

    pub fn err(self, e: &crate::contract::BrokerError) {
        let code = error_code(e);
        self.finish(Level::Error, Some(code));
    }

    /// WO-033: completes as a FAILURE at a chosen level — the record still says
    /// `ok:false` and still carries `error_code`, only the log level moves.
    ///
    /// For the rare call whose failure is the *expected, healthy* answer: the
    /// keyguard's self-forge probe (ADR-013) asks the database to reject a
    /// forged token, so its refusal is the good outcome and must not make a
    /// healthy boot cry `error`. Everything else keeps `err` — "failures always
    /// emit" (WO-010) is unchanged for every other caller.
    pub fn err_at(self, e: &crate::contract::BrokerError, level: Level) {
        let code = error_code(e);
        self.finish(level, Some(code));
    }

    fn finish(self, level: Level, code: Option<String>) {
        let dur_us = self.start.elapsed().as_micros() as u64;
        let mut fields = self.fields;
        fields.push(("dur_us", serde_json::json!(dur_us)));
        fields.push(("ok", serde_json::json!(code.is_none())));
        if let Some(c) = code {
            fields.push(("error_code", serde_json::json!(c)));
        }
        emit(level, self.event, &fields);
    }

    pub fn elapsed_ms(&self) -> f64 {
        self.start.elapsed().as_secs_f64() * 1e3
    }
}

// â”€â”€ metrics registry (Prometheus text) â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

const BUCKETS_MS: [f64; 8] = [1.0, 5.0, 10.0, 25.0, 50.0, 100.0, 250.0, 1000.0];

#[derive(Default)]
struct Histogram {
    buckets: [u64; BUCKETS_MS.len()],
    count: u64,
    sum_ms: f64,
}

#[derive(Default)]
struct Registry {
    histograms: HashMap<(String, String), Histogram>,
    counters: HashMap<(String, String), u64>,
    gauges: HashMap<(String, String), f64>,
}

fn registry() -> &'static Mutex<Registry> {
    static REG: OnceLock<Mutex<Registry>> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(Registry::default()))
}

/// Sorted, escaped `k="v"` label body. BTreeMap keeps series identity stable.
fn label_body(labels: &[(&str, &str)]) -> String {
    let map: BTreeMap<&str, &str> = labels.iter().copied().collect();
    map.iter()
        .map(|(k, v)| format!("{k}=\"{}\"", v.replace('\\', "\\\\").replace('"', "\\\"")))
        .collect::<Vec<_>>()
        .join(",")
}

pub fn observe_ms(name: &str, labels: &[(&str, &str)], ms: f64) {
    let key = (name.to_string(), label_body(labels));
    let mut reg = registry().lock().unwrap();
    let h = reg.histograms.entry(key).or_default();
    for (i, le) in BUCKETS_MS.iter().enumerate() {
        if ms <= *le {
            h.buckets[i] += 1;
        }
    }
    h.count += 1;
    h.sum_ms += ms;
}

pub fn inc(name: &str, labels: &[(&str, &str)], by: u64) {
    let key = (name.to_string(), label_body(labels));
    *registry().lock().unwrap().counters.entry(key).or_default() += by;
}

pub fn gauge(name: &str, labels: &[(&str, &str)], value: f64) {
    let key = (name.to_string(), label_body(labels));
    registry().lock().unwrap().gauges.insert(key, value);
}

/// Renders every series in Prometheus text exposition format (0.0.4).
pub fn render_prometheus() -> String {
    let reg = registry().lock().unwrap();
    let mut out = String::new();

    let mut hist_names: Vec<&String> = reg.histograms.keys().map(|(n, _)| n).collect();
    hist_names.sort_unstable();
    hist_names.dedup();
    for name in hist_names {
        out.push_str(&format!("# TYPE {name} histogram\n"));
        for ((n, labels), h) in reg.histograms.iter().filter(|((n, _), _)| n == name) {
            let sep = if labels.is_empty() { "" } else { "," };
            for (i, le) in BUCKETS_MS.iter().enumerate() {
                out.push_str(&format!(
                    "{n}_bucket{{{labels}{sep}le=\"{le}\"}} {}\n",
                    h.buckets[i]
                ));
            }
            out.push_str(&format!("{n}_bucket{{{labels}{sep}le=\"+Inf\"}} {}\n", h.count));
            let body = if labels.is_empty() { String::new() } else { format!("{{{labels}}}") };
            out.push_str(&format!("{n}_sum{body} {}\n", h.sum_ms));
            out.push_str(&format!("{n}_count{body} {}\n", h.count));
        }
    }

    let mut keys: Vec<&(String, String)> = reg.counters.keys().collect();
    keys.sort_unstable();
    let mut last = "";
    for key in keys {
        if key.0 != last {
            out.push_str(&format!("# TYPE {} counter\n", key.0));
            last = &key.0;
        }
        let body = if key.1.is_empty() { String::new() } else { format!("{{{}}}", key.1) };
        out.push_str(&format!("{}{body} {}\n", key.0, reg.counters[key]));
    }

    let mut keys: Vec<&(String, String)> = reg.gauges.keys().collect();
    keys.sort_unstable();
    let mut last = "";
    for key in keys {
        if key.0 != last {
            out.push_str(&format!("# TYPE {} gauge\n", key.0));
            last = &key.0;
        }
        let body = if key.1.is_empty() { String::new() } else { format!("{{{}}}", key.1) };
        out.push_str(&format!("{}{body} {}\n", key.0, reg.gauges[key]));
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trace_ids_are_unique_and_parseable() {
        let a = TraceId::new();
        let b = TraceId::new();
        assert_ne!(a, b);
        assert_eq!(TraceId::parse(a.as_str()).unwrap(), a);
        assert!(TraceId::parse("").is_none());
        assert!(TraceId::parse("bad id with spaces").is_none());
    }

    #[test]
    fn context_nests_and_restores() {
        let outer = TraceId::new();
        let g = enter(outer.clone(), "t1");
        assert_eq!(current_trace(), outer);
        assert_eq!(current_tenant(), "t1");
        {
            let inner = TraceId::new();
            let _g2 = enter(inner.clone(), "t2");
            assert_eq!(current_trace(), inner);
            assert_eq!(current_tenant(), "t2");
        }
        assert_eq!(current_trace(), outer);
        assert_eq!(current_tenant(), "t1");
        drop(g);
    }

    #[test]
    fn histogram_renders_prometheus_text() {
        observe_ms("frust_test_duration_ms", &[("verb", "db_read"), ("tenant", "tt")], 3.0);
        observe_ms("frust_test_duration_ms", &[("tenant", "tt"), ("verb", "db_read")], 30.0);
        let text = render_prometheus();
        assert!(text.contains("# TYPE frust_test_duration_ms histogram"));
        // label order is canonical, so both observations land in ONE series
        assert!(text.contains(r#"frust_test_duration_ms_bucket{tenant="tt",verb="db_read",le="5"} 1"#));
        assert!(text.contains(r#"frust_test_duration_ms_count{tenant="tt",verb="db_read"} 2"#));
    }

    #[test]
    fn error_codes_survive_extraction() {
        use crate::contract::BrokerError;
        assert_eq!(error_code(&BrokerError::IdentityUnresolved), "E_IDENTITY_UNRESOLVED");
        let db = BrokerError::Db { detail: "x THROW 'FRUST:E_DOCSTATUS:RESURRECTION' y".into() };
        assert_eq!(error_code(&db), "FRUST:E_DOCSTATUS:RESURRECTION");
        let wc = BrokerError::WriteConflictExhausted { attempts: 6 };
        assert_eq!(error_code(&wc), "E_WRITE_CONFLICT_EXHAUSTED");
    }
}

