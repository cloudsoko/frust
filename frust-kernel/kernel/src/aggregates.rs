//! ADR-010 Tier 2: worker-maintained rollups off the changefeed.
//!
//! Same algebra as the Tier-1 counters â€” a document's SIGNED CONTRIBUTION â€”
//! but computed kernel-side, for deltas an EVENT can't see (graph hops,
//! embedded-line diffs). The worker replays `SHOW CHANGES` from a persisted
//! versionstamp cursor; each batch applies its rollup deltas AND the cursor
//! advance in ONE transaction, so replay after a crash is exactly-once by
//! construction (the ADR-009 recovery story, with rollup state at stake).
//!
//! v3.2.0 feed shapes (probed):
//!   create: `{update: <full new doc>}`
//!   update: `{current: <after>, update: [<json-patch undo ops>]}` â€” applying
//!           the ops to `current` reconstructs the before-doc; `replace`/
//!           `add`/`remove` carry full old values (records, numbers, arrays);
//!           only plain strings degrade to a `change` text-diff op
//!   delete: `{delete: {id, original: <full before doc>}}` (INCLUDE ORIGINAL)

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};

use crate::contract::{BrokerError, Value};
use crate::decimal::Decimal;
use crate::telemetry;
use crate::db::Db;
use crate::surql;

/// Cursor records live here: `agg_cursor:<rollup>` = { vs, updated_at }.
/// Lag is data (ADR-010): dashboards read the cursor, not a log file.
pub const CURSOR_TABLE: &str = "agg_cursor";

/// One decoded feed entry: the doc before and after the mutation.
#[derive(Debug, Clone)]
pub struct Change {
    pub before: Option<serde_json::Value>,
    pub after: Option<serde_json::Value>,
}

/// Apply one diff-match-patch text patch (the feed's `change` op payload) to
/// a source string, yielding the pre-change string. Strict: every context and
/// deletion char is verified against the source â€” any mismatch is a loud
/// error, never a silently wrong reconstruction.
/// ponytail: char-indexed (dmp emits UTF-16 offsets); ASCII-safe, which
/// covers item codes/status strings â€” a verified mismatch fails loudly, so a
/// non-ASCII edge can never corrupt a rollup, only stop it.
fn dmp_unpatch(source: &str, patch: &str) -> Result<String, BrokerError> {
    let bad = |d: String| BrokerError::Db { detail: format!("text-diff undo: {d}") };
    let decode = |line: &str| -> Result<String, BrokerError> {
        let mut out = Vec::new();
        let b = line.as_bytes();
        let mut i = 0;
        while i < b.len() {
            if b[i] == b'%' && i + 2 < b.len() {
                let hex = std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("");
                out.push(u8::from_str_radix(hex, 16).map_err(|_| bad(format!("bad %-escape in '{line}'")))?);
                i += 3;
            } else {
                out.push(b[i]);
                i += 1;
            }
        }
        String::from_utf8(out).map_err(|_| bad("undecodable %-escape".into()))
    };
    let src: Vec<char> = source.chars().collect();
    let mut out = String::new();
    let mut pos = 0usize; // consumed source chars
    let mut lines = patch.lines().peekable();
    while let Some(header) = lines.next() {
        let h = header
            .strip_prefix("@@ -")
            .and_then(|s| s.split_once(" +"))
            .ok_or_else(|| bad(format!("bad hunk header '{header}'")))?;
        let (start1, len1) = match h.0.split_once(',') {
            Some((a, b)) => (
                a.parse::<usize>().map_err(|_| bad("bad hunk offset".into()))?,
                b.parse::<usize>().map_err(|_| bad("bad hunk length".into()))?,
            ),
            None => (h.0.parse::<usize>().map_err(|_| bad("bad hunk offset".into()))?, 1),
        };
        // dmp: 1-based start when the source length > 0; equal-to-position when 0
        let hunk_at = if len1 == 0 { start1 } else { start1.saturating_sub(1) };
        if hunk_at < pos || hunk_at > src.len() {
            return Err(bad(format!("hunk offset {hunk_at} out of order")));
        }
        out.extend(&src[pos..hunk_at]);
        pos = hunk_at;
        while let Some(line) = lines.peek() {
            if line.starts_with("@@") {
                break;
            }
            let line = lines.next().unwrap();
            let (tag, text) = line.split_at(1.min(line.len()));
            let text = decode(text)?;
            match tag {
                " " | "-" => {
                    let take: Vec<char> = text.chars().collect();
                    if src.get(pos..pos + take.len()) != Some(&take[..]) {
                        return Err(bad(format!("context mismatch at {pos}")));
                    }
                    pos += take.len();
                    if tag == " " {
                        out.push_str(&text);
                    }
                }
                "+" => out.push_str(&text),
                _ => return Err(bad(format!("bad hunk line '{line}'"))),
            }
        }
    }
    out.extend(&src[pos..]);
    Ok(out)
}

/// Reconstruct the before-doc by applying the feed's undo-patch to `current`.
/// `replace`/`add`/`remove` carry old values verbatim; `change` is a dmp text
/// patch on a string field, applied strictly (see `dmp_unpatch`).
fn apply_undo(current: &serde_json::Value, ops: &[serde_json::Value]) -> Result<serde_json::Value, BrokerError> {
    let mut doc = current.clone();
    for op in ops {
        let kind = op.get("op").and_then(|v| v.as_str()).unwrap_or("");
        let path = op.get("path").and_then(|v| v.as_str()).unwrap_or("");
        if !matches!(kind, "replace" | "add" | "remove" | "change") {
            return Err(BrokerError::Db {
                detail: format!("changefeed undo op '{kind}' at '{path}' is not reversible kernel-side"),
            });
        }
        let toks: Vec<String> = path
            .split('/')
            .skip(1)
            .map(|t| t.replace("~1", "/").replace("~0", "~"))
            .collect();
        let Some((last, parents)) = toks.split_last() else {
            doc = op.get("value").cloned().unwrap_or(serde_json::Value::Null);
            continue;
        };
        let mut cur = &mut doc;
        for t in parents {
            cur = match cur {
                serde_json::Value::Object(m) => m
                    .get_mut(t)
                    .ok_or_else(|| BrokerError::Db { detail: format!("undo path missing: {path}") })?,
                serde_json::Value::Array(a) => {
                    let i: usize = t
                        .parse()
                        .map_err(|_| BrokerError::Db { detail: format!("bad undo index in {path}") })?;
                    a.get_mut(i)
                        .ok_or_else(|| BrokerError::Db { detail: format!("undo index oob: {path}") })?
                }
                _ => return Err(BrokerError::Db { detail: format!("undo path through scalar: {path}") }),
            };
        }
        let value = || op.get("value").cloned().unwrap_or(serde_json::Value::Null);
        let unpatched = |slot: Option<&serde_json::Value>| -> Result<serde_json::Value, BrokerError> {
            let cur_s = slot.and_then(|v| v.as_str()).ok_or_else(|| BrokerError::Db {
                detail: format!("text-diff undo on non-string at {path}"),
            })?;
            let patch = op.get("value").and_then(|v| v.as_str()).ok_or_else(|| BrokerError::Db {
                detail: format!("text-diff undo without patch at {path}"),
            })?;
            Ok(serde_json::Value::String(dmp_unpatch(cur_s, patch)?))
        };
        match cur {
            serde_json::Value::Object(m) => match kind {
                "remove" => {
                    m.remove(last);
                }
                "change" => {
                    let v = unpatched(m.get(last.as_str()))?;
                    m.insert(last.clone(), v);
                }
                _ => {
                    m.insert(last.clone(), value());
                }
            },
            serde_json::Value::Array(a) => {
                let i: usize = if last == "-" {
                    a.len()
                } else {
                    last.parse()
                        .map_err(|_| BrokerError::Db { detail: format!("bad undo index in {path}") })?
                };
                match kind {
                    "replace" => {
                        *a.get_mut(i)
                            .ok_or_else(|| BrokerError::Db { detail: format!("undo index oob: {path}") })? =
                            value();
                    }
                    "change" => {
                        let v = unpatched(a.get(i))?;
                        a[i] = v;
                    }
                    "add" => a.insert(i.min(a.len()), value()),
                    _ => {
                        if i < a.len() {
                            a.remove(i);
                        }
                    }
                }
            }
            _ => return Err(BrokerError::Db { detail: format!("undo target is scalar: {path}") }),
        }
    }
    Ok(doc)
}

/// Decode SHOW CHANGES entries into before/after pairs + last versionstamp.
pub fn decode_entries(entries: &[serde_json::Value]) -> Result<(Vec<Change>, u64), BrokerError> {
    let mut out = Vec::new();
    let mut last_vs = 0u64;
    for e in entries {
        last_vs = e.get("versionstamp").and_then(|v| v.as_u64()).unwrap_or(last_vs);
        let empty = Vec::new();
        for c in e.get("changes").and_then(|v| v.as_array()).unwrap_or(&empty) {
            if c.get("define_table").is_some() {
                continue;
            }
            if let Some(d) = c.get("delete") {
                let before = d.get("original").cloned().ok_or_else(|| BrokerError::Db {
                    detail: "delete without original â€” source table needs CHANGEFEED ... INCLUDE ORIGINAL".into(),
                })?;
                out.push(Change { before: Some(before), after: None });
            } else if let Some(cur) = c.get("current") {
                let empty_ops = Vec::new();
                let ops = c.get("update").and_then(|v| v.as_array()).unwrap_or(&empty_ops);
                out.push(Change { before: Some(apply_undo(cur, ops)?), after: Some(cur.clone()) });
            } else if let Some(doc) = c.get("update").filter(|v| v.is_object()) {
                out.push(Change { before: None, after: Some(doc.clone()) });
            }
        }
    }
    Ok((out, last_vs))
}

/// What one document contributes to its rollup: (key, [(field, amount)]).
/// The worker adds the after-doc's contribution and subtracts the before-
/// doc's â€” exactness through edits, key-moves, and deletes falls out.
/// WO-025: `Send`, deliberately NOT `Sync`. Rollup contributors are moved onto
/// the single maintenance thread and never shared — `GroupRevenue` carries a
/// `RefCell` cache (a single-threaded-assumption artifact), which `Send` allows
/// and `Sync` would forbid. Requiring `Sync` here would be asking for a
/// guarantee the design does not need and the type cannot give.
pub trait Contrib: Send {
    /// Rollup table name; also the cursor record key.
    fn rollup(&self) -> &str;
    /// Source table whose changefeed feeds this rollup.
    fn source(&self) -> &str;
    fn contrib(&self, db: &Db, doc: &serde_json::Value)
        -> Result<Vec<(String, Vec<(String, Decimal)>)>, BrokerError>;
}

/// Revenue by customer group â€” the 2-hop key (WO-006 Q3, super-linear live).
/// The hop resolves kernel-side with a memoized customer -> group map; the
/// EVENT tier can't see it, the worker makes it one cached lookup.
pub struct GroupRevenue {
    pub source: String,
    pub rollup: String,
    /// Field on the linked customer holding the group (e.g. "grp").
    pub hop: String,
    cache: RefCell<HashMap<String, String>>,
}

impl GroupRevenue {
    pub fn new(source: &str, rollup: &str, hop: &str) -> Self {
        Self {
            source: source.into(),
            rollup: rollup.into(),
            hop: hop.into(),
            cache: RefCell::new(HashMap::new()),
        }
    }

    fn group_of(&self, db: &Db, customer: &str) -> Result<String, BrokerError> {
        if let Some(g) = self.cache.borrow().get(customer) {
            return Ok(g.clone());
        }
        let rid = surql::render_value(&Value::RecordId(customer.to_string()))?;
        if !self.hop.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(BrokerError::InvalidValue { detail: "bad hop field".into() });
        }
        let v = db.sql_root(&format!("SELECT VALUE <string>{} FROM ONLY {rid};", self.hop))?;
        let g = v.as_str().unwrap_or("(none)").to_string();
        self.cache.borrow_mut().insert(customer.to_string(), g.clone());
        Ok(g)
    }
}

impl Contrib for GroupRevenue {
    fn rollup(&self) -> &str {
        &self.rollup
    }
    fn source(&self) -> &str {
        &self.source
    }
    fn contrib(&self, db: &Db, doc: &serde_json::Value)
        -> Result<Vec<(String, Vec<(String, Decimal)>)>, BrokerError> {
        let Some(customer) = doc.get("customer").and_then(|v| v.as_str()) else {
            return Ok(Vec::new());
        };
        let total = numeric(doc.get("total"));
        let grp = self.group_of(db, customer)?;
        Ok(vec![(grp, vec![("revenue".into(), total), ("n".into(), Decimal::parse("1").unwrap())])])
    }
}

/// Item-wise sales from embedded lines â€” the shape with NO live door (the
/// flatten is contract-inexpressible, ADR-010): this rollup IS the report.
pub struct ItemSales {
    pub source: String,
    pub rollup: String,
}

impl ItemSales {
    pub fn new(source: &str, rollup: &str) -> Self {
        Self { source: source.into(), rollup: rollup.into() }
    }
}

impl Contrib for ItemSales {
    fn rollup(&self) -> &str {
        &self.rollup
    }
    fn source(&self) -> &str {
        &self.source
    }
    fn contrib(&self, _db: &Db, doc: &serde_json::Value)
        -> Result<Vec<(String, Vec<(String, Decimal)>)>, BrokerError> {
        let mut per_item: BTreeMap<String, (Decimal, Decimal)> = BTreeMap::new();
        let empty = Vec::new();
        for line in doc.get("lines").and_then(|v| v.as_array()).unwrap_or(&empty) {
            let Some(item) = line.get("item").and_then(|v| v.as_str()) else { continue };
            let e = per_item.entry(item.to_string()).or_insert((Decimal::ZERO, Decimal::ZERO));
            e.0 = e.0.add(numeric(line.get("qty")));
            e.1 = e.1.add(numeric(line.get("amount")));
        }
        Ok(per_item
            .into_iter()
            .map(|(k, (qty, amount))| (k, vec![("qty".into(), qty), ("amount".into(), amount)]))
            .collect())
    }
}

/// Reads a numeric line field that may arrive as a JSON number OR as a
/// DECIMAL, which SurrealDB serialises as a string ("150.00").
///
/// WO-015 found this the hard way: with real decimal amounts the old
/// `as_f64()` returned `None` and the rollup silently accumulated ZERO.
/// WO-016 finished the job â€” the value stays an exact [`Decimal`] from here
/// through the accumulator and into the written statement, so no money path
/// in this module touches `f64` (REQ-6.2.1).
fn numeric(v: Option<&serde_json::Value>) -> Decimal {
    Decimal::from_json(v)
}

/// The Tier-2 loop: replay feed from cursor -> fold signed contributions ->
/// apply deltas + advance cursor atomically.
pub struct RollupWorker<'a> {
    pub db: &'a Db,
    pub handler: Box<dyn Contrib>,
}

impl<'a> RollupWorker<'a> {
    pub fn new(db: &'a Db, handler: Box<dyn Contrib>) -> Result<Self, BrokerError> {
        // the only dynamic idents this module ever embeds â€” validated once
        for name in [handler.source(), handler.rollup()] {
            if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                return Err(BrokerError::InvalidValue { detail: format!("bad rollup table name: {name}") });
            }
        }
        // lag is queryable by any signed-in user; only the kernel writes it
        db.sql_root(&format!(
            "DEFINE TABLE IF NOT EXISTS {CURSOR_TABLE} SCHEMALESS \
             PERMISSIONS FOR select WHERE $auth.id != NONE FOR create, update, delete NONE;"
        ))?;
        Ok(Self { db, handler })
    }

    fn cursor_id(&self) -> String {
        format!("{CURSOR_TABLE}:{}", self.handler.rollup())
    }

    /// Last processed versionstamp (0 = never ran).
    pub fn cursor(&self) -> Result<u64, BrokerError> {
        let v = self.db.sql_root(&format!("SELECT VALUE vs FROM {};", self.cursor_id()))?;
        Ok(v.as_array().and_then(|a| a.first()).and_then(|x| x.as_u64()).unwrap_or(0))
    }

    /// Pending feed entries beyond the cursor (capped at 1000) â€” the
    /// queryable-lag number, derived from the same cursor record dashboards
    /// can read directly.
    pub fn lag(&self) -> Result<usize, BrokerError> {
        let since = self.cursor()? + 1;
        let v = self.db.sql_root(&format!(
            "SHOW CHANGES FOR TABLE {} SINCE {since} LIMIT 1000;",
            self.handler.source()
        ))?;
        let pending =
            v.as_array().map(|a| a.iter().filter(|e| e.get("changes").is_some()).count()).unwrap_or(0);
        crate::telemetry::gauge(
            "frust_rollup_lag",
            &[("rollup", self.handler.rollup()), ("tenant", self.db.tenant_id())],
            pending as f64,
        );
        Ok(pending)
    }

    /// Process up to `limit` feed entries. Returns decoded changes applied
    /// (0 = caught up). Deltas and cursor advance commit in one transaction:
    /// a crash before COMMIT replays the batch, never half of it.
    pub fn tick(&self, limit: usize) -> Result<usize, BrokerError> {
        let since = self.cursor()? + 1;
        let v = self.db.sql_root(&format!(
            "SHOW CHANGES FOR TABLE {} SINCE {since} LIMIT {limit};",
            self.handler.source()
        ))?;
        let entries = v.as_array().cloned().unwrap_or_default();
        if entries.is_empty() {
            // caught up: the lag gauge says so without another feed read
            crate::telemetry::gauge(
                "frust_rollup_lag",
                &[("rollup", self.handler.rollup()), ("tenant", self.db.tenant_id())],
                0.0,
            );
            return Ok(0);
        }
        let (changes, last_vs) = decode_entries(&entries)?;
        let mut acc: BTreeMap<String, BTreeMap<String, Decimal>> = BTreeMap::new();
        // the signed fold is EXACT: subtract the before-contribution, add the
        // after-contribution, all in decimal (REQ-6.2.1 â€” an f64 here would
        // reintroduce the error the requirement exists to prevent)
        for ch in &changes {
            for (doc, subtract) in [(&ch.before, true), (&ch.after, false)] {
                if let Some(doc) = doc {
                    for (key, fields) in self.handler.contrib(self.db, doc)? {
                        let bucket = acc.entry(key).or_default();
                        for (f, amt) in fields {
                            let signed = if subtract { amt.neg() } else { amt };
                            let slot = bucket.entry(f).or_insert(Decimal::ZERO);
                            *slot = slot.add(signed);
                        }
                    }
                }
            }
        }
        let rollup = self.handler.rollup();
        let mut q = String::from("BEGIN; ");
        for (key, fields) in &acc {
            // `dec`-suffixed literals against a decimal column: v3.2.0 keeps
            // decimal through `+` (probed), and int/float operands promote TO
            // decimal rather than degrading it
            let sets: Vec<String> = fields
                .iter()
                .map(|(f, d)| format!("{f} = ({f} ?? 0dec) + {}dec", d.to_plain_string()))
                .collect();
            q.push_str(&format!(
                "UPSERT type::record('{rollup}', '{}') SET k = '{0}', {}; ",
                surql::escape_str(key),
                sets.join(", ")
            ));
        }
        q.push_str(&format!(
            "UPSERT {} SET vs = {last_vs}, updated_at = time::now(); COMMIT;",
            self.cursor_id()
        ));
        self.db.sql_root(&q)?;
        Ok(changes.len())
    }

    /// WO-016 criterion 4: the upgrade path for rollups written before money
    /// was decimal.
    ///
    /// Rollups are DERIVED data, so the upgrade is **recompute from source**,
    /// never a conversion of the stored floats â€” converting would launder the
    /// float error into a decimal that looks exact and is not. This resets
    /// the rollup docs and the cursor, so the next drain rebuilds every
    /// bucket from the changefeed in decimal.
    ///
    /// Cost is one full feed replay per rollup: a one-time upgrade whose
    /// result is reconcilable against the source (which is exactly what the
    /// WO-007 reconciliation tests assert, now decimal-exact).
    pub fn recompute_from_source(&self) -> Result<usize, BrokerError> {
        let rollup = self.handler.rollup();
        self.db.sql_root(&format!(
            "BEGIN; DELETE {rollup}; UPSERT {} SET vs = 0, updated_at = time::now(); COMMIT;",
            self.cursor_id()
        ))?;
        telemetry::emit(
            telemetry::Level::Info,
            "rollup_recompute_reset",
            &[("rollup", serde_json::json!(rollup))],
        );
        self.drain()
    }

    /// Tick until caught up; returns total changes applied.
    /// (v3.2.0 quirk: SHOW CHANGES LIMIT is not linear in entries â€” small
    /// values can return nothing at a nonzero cursor. 1000 is probed-good;
    /// smaller ticks are for deliberately partial batches in tests.)
    pub fn drain(&self) -> Result<usize, BrokerError> {
        let mut total = 0;
        loop {
            let n = self.tick(1000)?;
            if n == 0 {
                return Ok(total);
            }
            total += n;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feed shapes exactly as probed on v3.2.0.
    #[test]
    fn decode_reconstructs_before_docs() {
        let entries: Vec<serde_json::Value> = serde_json::from_str(
            r#"[
            {"versionstamp": 10, "changes": [{"update": {"id": "src:l1", "customer": "customer:42", "total": 100.0,
                "lines": [{"item": "a", "qty": 2, "amount": 60.0}, {"item": "b", "qty": 1, "amount": 40.0}]}}]},
            {"versionstamp": 20, "changes": [{"current": {"id": "src:l1", "customer": "customer:42", "total": 90.0,
                "lines": [{"item": "a", "qty": 3, "amount": 90.0}]},
                "update": [
                    {"op": "replace", "path": "/lines/0/amount", "value": 60.0},
                    {"op": "replace", "path": "/lines/0/qty", "value": 2},
                    {"op": "add", "path": "/lines/1", "value": {"item": "b", "qty": 1, "amount": 40.0}},
                    {"op": "replace", "path": "/total", "value": 100.0}]}]},
            {"versionstamp": 30, "changes": [{"delete": {"id": "src:l1", "original": {"id": "src:l1",
                "customer": "customer:77", "total": 90.0, "lines": [{"item": "a", "qty": 3, "amount": 90.0}]}}}]}
        ]"#,
        )
        .unwrap();
        let (changes, last_vs) = decode_entries(&entries).unwrap();
        assert_eq!(last_vs, 30);
        assert_eq!(changes.len(), 3);
        // create: no before
        assert!(changes[0].before.is_none());
        assert_eq!(changes[0].after.as_ref().unwrap()["total"], 100.0);
        // update: before reconstructed â€” old total, both original lines back
        let before = changes[1].before.as_ref().unwrap();
        assert_eq!(before["total"], 100.0);
        assert_eq!(before["lines"].as_array().unwrap().len(), 2);
        assert_eq!(before["lines"][1]["item"], "b");
        assert_eq!(changes[1].after.as_ref().unwrap()["lines"].as_array().unwrap().len(), 1);
        // delete: full original, no after
        assert!(changes[2].after.is_none());
        assert_eq!(changes[2].before.as_ref().unwrap()["customer"], "customer:77");
    }

    /// A `change` op (dmp text patch, exactly as probed on v3.2.0) reverses
    /// string fields; a context mismatch fails LOUDLY, never silently wrong.
    #[test]
    fn text_diff_undo_applies_strictly() {
        let cur = serde_json::json!({"month": "2026-08"});
        let ops = vec![serde_json::json!({"op": "change", "path": "/month", "value": "@@ -3,5 +3,5 @@\n 26-0\n-8\n+7\n"})];
        let before = apply_undo(&cur, &ops).unwrap();
        assert_eq!(before["month"], "2026-07");

        let tampered = serde_json::json!({"month": "2099-99"});
        let err = apply_undo(&tampered, &ops).unwrap_err();
        assert!(err.to_string().contains("mismatch"), "{err}");

        // nested string change inside a line object
        let cur = serde_json::json!({"lines": [{"item": "gadget", "qty": 1}]});
        let ops = vec![serde_json::json!({"op": "change", "path": "/lines/0/item",
            "value": "@@ -1,6 +1,6 @@\n-gadget\n+widget\n"})];
        let before = apply_undo(&cur, &ops).unwrap();
        assert_eq!(before["lines"][0]["item"], "widget");
    }
}

