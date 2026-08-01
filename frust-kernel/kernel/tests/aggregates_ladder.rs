//! WO-007: the ADR-010 aggregates ladder, Tiers 1-2, proven.
//!
//! Functional proofs run against the dev instance (:8899, ns frust). The 1 M
//! before/after benchmark runs against the WO-006 fixture (:8898), ignored by
//! default:
//!
//!   cargo test --release --test aggregates_ladder -- --ignored --nocapture
//!
//! The named escalation lives in `tier1_monthly_counter_exact_under_burst`:
//! if EVENT increments lose updates under concurrent submits, reconciliation
//! fails there with an ESCALATION-tagged message.

use std::collections::BTreeMap;
use std::sync::Arc;

use frust_kernel::aggregates::{GroupRevenue, ItemSales, RollupWorker};
use frust_kernel::broker::{Broker, Caller, HookChain, HookDispatch};
use frust_kernel::contract::*;
use frust_kernel::db::{scoped_db, Db};
use frust_kernel::tenancy::{single_tenant, ResolvedTenant};
use frust_kernel::sync::{backfill_counter, counter_event_ddl, AggregateDef, DocTypeDef, MetadataSync};

/// Pass-through dispatcher: hooks still fire on every write (ADR-006 edge 6),
/// they just don't mutate â€” WO-007 proves aggregate exactness, not hook flow.
struct PassHooks;
impl HookDispatch for PassHooks {
    fn validate(&self, doctype: &str, doc: &[(String, Value)]) -> Result<Vec<(String, Value)>, BrokerError> {
        Ok(doc.to_vec())
    }
}

fn fresh(name: &str) -> (Db, ResolvedTenant) {
    let cfg = single_tenant(&name.to_string()).expect("tenancy");
    let db = scoped_db(&cfg);
    db.sql_root_ns(&format!("REMOVE DATABASE IF EXISTS {name}; DEFINE DATABASE {name};"))
        .expect("provision test database");
    // the kernel-owned identity posture (WO-008) â€” tests consume it, never
    // hand-roll it
    db.sql_root(&format!(
        "{}; {}; \
         CREATE app_user:boss  SET name = 'boss',  role = 'manager', pass = crypto::argon2::generate('pw-boss'); \
         CREATE app_user:clerk SET name = 'clerk', role = 'user',    pass = crypto::argon2::generate('pw-clerk'); \
         DEFINE TABLE doctype SCHEMALESS PERMISSIONS NONE;",
        frust_kernel::meta::identity_ddl(),
        frust_kernel::meta::access_ddl(),
    ))
    .unwrap();
    (db, cfg)
}

fn boss() -> Caller {
    Caller { user: "boss".into(), pass: "pw-boss".into(), role: "manager".into() }
}
fn clerk() -> Caller {
    Caller { user: "clerk".into(), pass: "pw-clerk".into(), role: "user".into() }
}
fn broker(cfg: &ResolvedTenant) -> Broker {
    Broker::new(scoped_db(&cfg), Box::new(PassHooks))
}

/// Live GROUP BY (root) vs rollup docs: every live bucket matches exactly;
/// rollup buckets absent from live must have fully reversed to ~0.
/// WO-016: reconciliation is DECIMAL-EXACT, not epsilon-close.
///
/// Money now travels as decimal (JSON strings), counts as decimal too, so
/// the old `as_f64()` + `1e-6` comparison would both misread the values and
/// accept drift. `Decimal::from_json` reads either representation and the
/// comparison is exact equality â€” which is the whole point of REQ-6.2.1: an
/// epsilon here would hide precisely the defect the requirement forbids.
fn reconcile(db: &Db, live_q: &str, rollup: &str, metric: &str, tag: &str) {
    use frust_kernel::decimal::Decimal;

    let live = db.sql_root(live_q).unwrap();
    let mut expect: BTreeMap<String, (Decimal, Decimal)> = BTreeMap::new();
    for row in live.as_array().unwrap() {
        let k = match &row["k"] {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        expect.insert(
            k,
            (Decimal::from_json(row.get("n")), Decimal::from_json(row.get(metric))),
        );
    }
    let got = db.sql_root(&format!("SELECT k, n, {metric} FROM {rollup};")).unwrap();
    let mut seen = 0;
    for row in got.as_array().unwrap() {
        let k = row["k"].as_str().unwrap_or("?").to_string();
        let n = Decimal::from_json(row.get("n"));
        let m = Decimal::from_json(row.get(metric));
        match expect.get(&k) {
            Some((en, em)) => {
                assert_eq!(
                    n, *en,
                    "{tag}: ESCALATION-CHECK count drift on bucket {k}: {} vs {}",
                    n.to_plain_string(), en.to_plain_string()
                );
                assert_eq!(
                    m, *em,
                    "{tag}: ESCALATION-CHECK {metric} drift on {k}: rollup {} vs live {}",
                    m.to_plain_string(), em.to_plain_string()
                );
                seen += 1;
            }
            None => {
                assert!(n.is_zero(), "{tag}: dead bucket {k} not fully reversed (n={})", n.to_plain_string());
                assert!(m.is_zero(), "{tag}: dead bucket {k} holds {metric}={}", m.to_plain_string());
            }
        }
    }
    assert_eq!(seen, expect.len(), "{tag}: rollup is missing live buckets");
}

// â”€â”€ Tier 1 â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

/// Criterion 1: counter declared in metadata -> compiled through the sync
/// engine -> exact after a concurrent submit burst. THE NAMED ESCALATION
/// GATE: a lost EVENT increment fails reconciliation here.
#[test]
fn tier1_monthly_counter_exact_under_burst() {
    let (db, cfg) = fresh("agg_t1");
    db.sql_root(r#"CREATE doctype CONTENT {
        name: 'sinv3', submittable: true,
        fields: [ { fieldname: 'month', fieldtype: 'Data' }, { fieldname: 'total', fieldtype: 'Currency' } ],
        aggregates: [ { kind: 'counter', rollup: 'si_monthly', key: 'month',
                        metrics: [ { name: 'revenue', field: 'total' } ] } ]
    };"#).unwrap();
    db.sql_root(r#"CREATE doctype CONTENT {
        name: 'si_monthly',
        fields: [ { fieldname: 'k', fieldtype: 'Data' }, { fieldname: 'revenue', fieldtype: 'Currency' }, { fieldname: 'n', fieldtype: 'Data' } ]
    };"#).unwrap();
    let applied = MetadataSync { base: cfg.clone() }.sync(&db).expect("sync").applied;
    assert_eq!(applied, 2, "source + rollup doctypes applied");

    // concurrent submits: 6 threads x 20, three months interleaved so every
    // rollup doc is contended across threads
    let b = Arc::new(broker(&cfg));
    let months = ["2026-01", "2026-02", "2026-03"];
    let handles: Vec<_> = (0..6)
        .map(|t| {
            let b = Arc::clone(&b);
            std::thread::spawn(move || {
                let caller = clerk();
                for i in 0..20 {
                    let doc = vec![
                        ("month".to_string(), Value::Text(months[(t + i) % 3].into())),
                        ("total".to_string(), Value::Float((t * 20 + i) as f64 + 0.5)),
                        ("docstatus".to_string(), Value::Int(1)),
                    ];
                    b.db_write(&caller, &HookChain::default(), WriteOp::Create, "sinv3", None, &doc)
                        .expect("burst submit");
                }
            })
        })
        .collect();
    for h in handles {
        h.join().expect("burst thread");
    }
    println!("burst done; conflict retries observed by test broker: {}", b.db.conflict_retries());

    reconcile(
        &db,
        "SELECT month AS k, count() AS n, math::sum(total) AS revenue FROM sinv3 WHERE docstatus = 1 GROUP BY k;",
        "si_monthly",
        "revenue",
        "tier1-monthly",
    );

    // criterion 5: the rollup is a DocType â€” manager reads it through the
    // contract; clerk's session sees zero rows; clerk cannot tamper
    let rows = b.db_read(&boss(), "si_monthly", None, &[], &ReadOpts::default()).unwrap();
    assert_eq!(rows.len(), 3, "manager reads 3 rollup docs through the broker");
    assert!(rows.iter().all(|r| r.get("revenue").is_some() && r.get("k").is_some()));
    let hidden = b.db_read(&clerk(), "si_monthly", None, &[], &ReadOpts::default()).unwrap();
    assert!(hidden.is_empty(), "row permission hides rollups from non-managers");
    db.sql_as("clerk", "pw-clerk", "UPSERT si_monthly:evil SET revenue = 999999;").unwrap();
    let n = db.sql_root("SELECT count() FROM si_monthly GROUP ALL;").unwrap();
    assert_eq!(n.as_array().unwrap()[0]["count"].as_i64(), Some(3), "tamper write matched nothing");
}

/// Criterion 2: the AR delta counter â€” docstatus 2 REVERSES the contribution
/// (the clause separating an ERP counter from a toy), proven by full-scan
/// reconciliation after a mixed submit/pay/cancel burst.
#[test]
fn tier1_ar_counter_reverses_on_cancel() {
    let (db, cfg) = fresh("agg_t2");
    db.sql_root(r#"CREATE doctype CONTENT { name: 'cust4', fields: [ { fieldname: 'cname', fieldtype: 'Data' } ] };"#).unwrap();
    db.sql_root(r#"CREATE doctype CONTENT {
        name: 'sinv4', submittable: true,
        fields: [ { fieldname: 'customer', fieldtype: 'Link', options: ['cust4'] },
                  { fieldname: 'outstanding', fieldtype: 'Currency' } ],
        aggregates: [ { kind: 'counter', rollup: 'ar_by_customer', key: 'customer',
                        metrics: [ { name: 'outstanding', field: 'outstanding' } ] } ]
    };"#).unwrap();
    db.sql_root(r#"CREATE doctype CONTENT {
        name: 'ar_by_customer',
        fields: [ { fieldname: 'k', fieldtype: 'Data' }, { fieldname: 'outstanding', fieldtype: 'Currency' }, { fieldname: 'n', fieldtype: 'Data' } ]
    };"#).unwrap();
    MetadataSync { base: cfg.clone() }.sync(&db).expect("sync");
    for c in 0..5 {
        db.sql_root(&format!("CREATE cust4:c{c} SET cname = 'c{c}';")).unwrap();
    }

    // mixed burst: 4 threads create submitted invoices, then cancel every
    // 3rd of their own and post a payment against every 5th
    let b = Arc::new(broker(&cfg));
    let handles: Vec<_> = (0..4)
        .map(|t| {
            let b = Arc::clone(&b);
            std::thread::spawn(move || {
                let (clerk, boss) = (clerk(), boss());
                let mut ids = Vec::new();
                for i in 0..15 {
                    let doc = vec![
                        ("customer".to_string(), Value::RecordId(format!("cust4:c{}", (t * 15 + i) % 5))),
                        ("outstanding".to_string(), Value::Float(10.0 + i as f64)),
                        ("docstatus".to_string(), Value::Int(1)),
                    ];
                    let out = b
                        .db_write(&clerk, &HookChain::default(), WriteOp::Create, "sinv4", None, &doc)
                        .expect("create");
                    ids.push(out["id"].as_str().unwrap().split_once(':').unwrap().1.to_string());
                }
                for (i, id) in ids.iter().enumerate() {
                    if i % 3 == 0 {
                        b.db_write(&boss, &HookChain::default(), WriteOp::Update, "sinv4", Some(id),
                            &[("docstatus".to_string(), Value::Int(2))]).expect("cancel");
                    } else if i % 5 == 1 {
                        b.db_write(&boss, &HookChain::default(), WriteOp::Update, "sinv4", Some(id),
                            &[("outstanding".to_string(), Value::Float(3.25))]).expect("payment");
                    }
                }
            })
        })
        .collect();
    for h in handles {
        h.join().expect("burst thread");
    }

    // cancel EVERYTHING still open for c0: its bucket must reverse to zero
    let open = db
        .sql_root("SELECT VALUE id FROM sinv4 WHERE customer = cust4:c0 AND docstatus = 1;")
        .unwrap();
    let boss_c = boss();
    for id in open.as_array().unwrap() {
        let key = id.as_str().unwrap().split_once(':').unwrap().1.to_string();
        b.db_write(&boss_c, &HookChain::default(), WriteOp::Update, "sinv4", Some(&key),
            &[("docstatus".to_string(), Value::Int(2))]).expect("final cancel");
    }

    reconcile(
        &db,
        "SELECT <string>customer AS k, count() AS n, math::sum(outstanding) AS outstanding \
         FROM sinv4 WHERE docstatus = 1 GROUP BY k;",
        "ar_by_customer",
        "outstanding",
        "tier1-ar",
    );
    let c0 = db.sql_root("SELECT n, outstanding FROM ar_by_customer WHERE k = 'cust4:c0';").unwrap();
    let c0 = &c0.as_array().unwrap()[0];
    // decimal-exact reversal: EXACTLY zero, not "close to" zero (WO-016)
    assert!(
        frust_kernel::decimal::Decimal::from_json(c0.get("n")).is_zero(),
        "fully-canceled customer: count reversed to exactly 0"
    );
    assert!(
        frust_kernel::decimal::Decimal::from_json(c0.get("outstanding")).is_zero(),
        "fully-canceled customer: AR reversed to exactly 0"
    );
}

// â”€â”€ Tier 2 â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

/// Criterion 3: the 2-hop group rollup off the changefeed â€” reconciliation
/// after a mixed burst, queryable lag, and a mid-stream worker restart that
/// loses nothing (cursor replay; deltas + cursor commit atomically).
#[test]
fn tier2_group_revenue_rollup_with_restart() {
    let (db, cfg) = fresh("agg_t3");
    db.sql_root(r#"CREATE doctype CONTENT { name: 'grp5', fields: [ { fieldname: 'gname', fieldtype: 'Data' } ] };"#).unwrap();
    db.sql_root(r#"CREATE doctype CONTENT {
        name: 'cust5', fields: [ { fieldname: 'grp', fieldtype: 'Link', options: ['grp5'] } ] };"#).unwrap();
    db.sql_root(r#"CREATE doctype CONTENT {
        name: 'src5',
        fields: [ { fieldname: 'customer', fieldtype: 'Link', options: ['cust5'] },
                  { fieldname: 'total', fieldtype: 'Currency' } ],
        aggregates: [ { kind: 'worker', rollup: 'rev_by_group', handler: 'group_revenue' } ]
    };"#).unwrap();
    db.sql_root(r#"CREATE doctype CONTENT {
        name: 'rev_by_group',
        fields: [ { fieldname: 'k', fieldtype: 'Data' }, { fieldname: 'revenue', fieldtype: 'Currency' }, { fieldname: 'n', fieldtype: 'Data' } ]
    };"#).unwrap();
    MetadataSync { base: cfg.clone() }.sync(&db).expect("sync");
    for g in 0..3 {
        db.sql_root(&format!("CREATE grp5:g{g} SET gname = 'g{g}';")).unwrap();
    }
    for c in 0..6 {
        db.sql_root(&format!("CREATE cust5:c{c} SET grp = grp5:g{};", c % 3)).unwrap();
    }

    // batch A: creates + edits + a cross-group customer move + a delete
    for i in 0..20 {
        db.sql_root(&format!("CREATE src5:a{i} SET customer = cust5:c{}, total = {}.0;", i % 6, 10 + i)).unwrap();
    }
    db.sql_root("UPDATE src5:a3 SET total = 500.0;").unwrap();
    db.sql_root("UPDATE src5:a4 SET customer = cust5:c5;").unwrap(); // g1 -> g2
    db.sql_root("DELETE src5:a5;").unwrap();

    // worker #1 processes only part of the feed, then "crashes" (dropped).
    // (SHOW CHANGES with a tiny LIMIT can return only the leading DDL entry,
    // which decodes to zero doc-changes while still advancing the cursor â€”
    // tick again until real changes land.)
    {
        let w1 = RollupWorker::new(&db, Box::new(GroupRevenue::new("src5", "rev_by_group", "grp"))).unwrap();
        assert!(w1.lag().unwrap() > 0, "lag visible before processing");
        let mut part = 0;
        for _ in 0..4 {
            part = w1.tick(15).unwrap();
            if part > 0 {
                break;
            }
        }
        assert!(part > 0, "partial batch processed");
        assert!(w1.lag().unwrap() > 0, "feed not fully consumed before the crash");
    }

    // batch B lands while no worker is running
    for i in 0..10 {
        db.sql_root(&format!("CREATE src5:b{i} SET customer = cust5:c{}, total = {}.0;", i % 6, 100 + i)).unwrap();
    }

    // worker #2 resumes from the persisted cursor â€” nothing lost, nothing doubled
    let w2 = RollupWorker::new(&db, Box::new(GroupRevenue::new("src5", "rev_by_group", "grp"))).unwrap();
    w2.drain().unwrap();
    assert_eq!(w2.lag().unwrap(), 0, "caught up after restart");
    let cur = db.sql_root("SELECT vs, updated_at FROM agg_cursor:rev_by_group;").unwrap();
    assert!(cur.as_array().unwrap()[0]["vs"].as_u64().unwrap() > 0, "cursor position is queryable data");

    reconcile(
        &db,
        "SELECT <string>customer.grp AS k, count() AS n, math::sum(total) AS revenue FROM src5 GROUP BY k;",
        "rev_by_group",
        "revenue",
        "tier2-group",
    );

    // staleness is honest: new writes -> lag > 0 until the next drain
    db.sql_root("CREATE src5:late SET customer = cust5:c0, total = 7.0;").unwrap();
    assert!(w2.lag().unwrap() > 0, "lag reflects pending feed entries");
    w2.drain().unwrap();
    assert_eq!(w2.lag().unwrap(), 0);
}

/// Criterion 4: item-wise rollup from embedded-line diffs â€” the shape with NO
/// live door. Acceptance: the report EXISTS (readable through the contract)
/// and reconciles against a root full-scan flatten.
#[test]
fn tier2_item_rollup_from_line_diffs() {
    let (db, cfg) = fresh("agg_t4");
    db.sql_root(r#"CREATE doctype CONTENT {
        name: 'src6',
        fields: [ { fieldname: 'lines', fieldtype: 'Table' } ],
        aggregates: [ { kind: 'worker', rollup: 'sales_by_item', handler: 'item_sales' } ]
    };"#).unwrap();
    db.sql_root(r#"CREATE doctype CONTENT {
        name: 'sales_by_item',
        fields: [ { fieldname: 'k', fieldtype: 'Data' }, { fieldname: 'qty', fieldtype: 'Data' }, { fieldname: 'amount', fieldtype: 'Currency' } ]
    };"#).unwrap();
    MetadataSync { base: cfg.clone() }.sync(&db).expect("sync");

    let items = ["widget", "gadget", "sprocket", "flange"];
    for i in 0..15 {
        let (a, b) = (items[i % 4], items[(i + 1) % 4]);
        db.sql_root(&format!(
            "CREATE src6:d{i} SET lines = [ \
             {{ item: '{a}', qty: {}, amount: {}.0 }}, \
             {{ item: '{b}', qty: 1, amount: 5.0 }} ];",
            1 + i % 3,
            10 + i
        ))
        .unwrap();
    }
    // line-level churn: qty change, line dropped, whole array replaced, doc deleted
    db.sql_root("UPDATE src6:d0 SET lines = [ { item: 'widget', qty: 9, amount: 90.0 } ];").unwrap();
    db.sql_root("UPDATE src6:d1 SET lines = [ { item: 'flange', qty: 2, amount: 8.0 }, { item: 'widget', qty: 1, amount: 3.0 } ];").unwrap();
    db.sql_root("DELETE src6:d2;").unwrap();

    let w = RollupWorker::new(&db, Box::new(ItemSales::new("src6", "sales_by_item"))).unwrap();
    w.drain().unwrap();

    // reconcile against the contract-inexpressible flatten (root-only shape)
    let live = db
        .sql_root(
            "SELECT item AS k, math::sum(qty) AS qty, math::sum(amount) AS amount \
             FROM (SELECT VALUE lines FROM src6).flatten() GROUP BY k;",
        )
        .unwrap();
    let mut expect: BTreeMap<String, (f64, f64)> = BTreeMap::new();
    for row in live.as_array().unwrap() {
        expect.insert(
            row["k"].as_str().unwrap().to_string(),
            (row["qty"].as_f64().unwrap(), row["amount"].as_f64().unwrap()),
        );
    }
    let got = db.sql_root("SELECT k, qty, amount FROM sales_by_item;").unwrap();
    for row in got.as_array().unwrap() {
        use frust_kernel::decimal::Decimal;
        let k = row["k"].as_str().unwrap();
        let (eq, ea) = expect.remove(k).unwrap_or((0.0, 0.0));
        // decimal-exact against the live flatten (WO-016 criterion 3)
        assert_eq!(
            Decimal::from_json(row.get("qty")),
            Decimal::parse(&eq.to_string()).unwrap_or(Decimal::ZERO),
            "qty drift on {k}"
        );
        assert_eq!(
            Decimal::from_json(row.get("amount")),
            Decimal::parse(&ea.to_string()).unwrap_or(Decimal::ZERO),
            "amount drift on {k}"
        );
    }
    assert!(expect.is_empty(), "rollup missing items: {expect:?}");

    // the report exists through the contract â€” that IS the acceptance
    let b = broker(&cfg);
    let rows = b.db_read(&boss(), "sales_by_item", None, &[], &ReadOpts::default()).unwrap();
    assert_eq!(rows.len(), 4, "item-wise report served from rollup DocTypes");
}

// â”€â”€ The 1 M before/after â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

/// Criterion 1's headline on the WO-006 fixture: monthly revenue 7.7 s live
/// -> rollup read < 100 ms, exact after a concurrent burst at 1 M.
#[test]
#[ignore = "WO-007 scale leg: needs the WO-006 fixture instance on :8898"]
fn tier1_counter_at_1m() {
    let cfg = frust_kernel::tenancy::Tenancy::from_config(
        frust_kernel::db::ConnConfig {
            endpoint: "http://127.0.0.1:8898".into(),
            ..Default::default()
        },
        frust_kernel::tenancy::TenancyConfig {
            strategy: "database-per-tenant",
            namespace: Some("scale"),
            database: None,
            environment: None,
            tenants: &["erp"],
        },
    )
    .expect("tenancy")
    .sole()
    .expect("sole tenant");
    let db = scoped_db(&cfg);
    let n = db.sql_root("SELECT count() FROM sales_invoice GROUP ALL;").unwrap();
    let n = n.as_array().unwrap()[0]["count"].as_i64().unwrap();
    println!("=== WO-007 1M leg: {n} invoices, {} build ===", if cfg!(debug_assertions) { "DEBUG" } else { "RELEASE" });

    // rollup table (write-closed) + broker metadata + the generated EVENT
    db.sql_root(
        "DEFINE TABLE OVERWRITE si_monthly_1m SCHEMALESS \
         PERMISSIONS FOR select WHERE $auth.role = 'manager' FOR create, update, delete NONE; \
         DELETE doctype WHERE name = 'si_monthly_1m'; \
         CREATE doctype CONTENT { name: 'si_monthly_1m', fields: [ \
           { fieldname: 'k', fieldtype: 'Data' }, { fieldname: 'revenue', fieldtype: 'Currency' }, { fieldname: 'n', fieldtype: 'Data' } ] };",
    )
    .unwrap();
    let dt = DocTypeDef {
        name: "sales_invoice".into(),
        app: None,
        submittable: false,
        fields: vec![],
        aggregates: vec![],
    };
    let agg: AggregateDef = serde_json::from_value(serde_json::json!({
        "kind": "counter", "rollup": "si_monthly_1m", "key": "month",
        "metrics": [ { "name": "revenue", "field": "total" } ]
    }))
    .unwrap();
    db.sql_root(&counter_event_ddl(&dt, &agg).unwrap()).unwrap();

    let t = std::time::Instant::now();
    let buckets = backfill_counter(&db, &dt, &agg).unwrap();
    println!("backfill over {n} rows: {} ms -> {buckets} rollup docs", t.elapsed().as_millis());

    // the before/after: monthly revenue through the broker, record principal
    let b = Arc::new(Broker::new(scoped_db(&cfg), Box::new(PassHooks)));
    let analyst = Caller { user: "analyst".into(), pass: "pw-analyst".into(), role: "manager".into() };
    let mut times = Vec::new();
    for _ in 0..3 {
        let t = std::time::Instant::now();
        let rows = b.db_read(&analyst, "si_monthly_1m", None, &[], &ReadOpts::default()).unwrap();
        times.push(t.elapsed().as_millis());
        assert_eq!(rows.len() as u64, buckets);
    }
    times.sort_unstable();
    println!("monthly report from rollups (broker, record user): {times:?} ms (live was ~7700 ms)");
    assert!(times[1] < 100, "criterion 1: rollup report read must be < 100 ms, got {} ms", times[1]);

    // concurrent burst through the broker at 1 M, then full reconciliation
    let handles: Vec<_> = (0..4)
        .map(|t| {
            let b = Arc::clone(&b);
            std::thread::spawn(move || {
                let caller = Caller { user: "analyst".into(), pass: "pw-analyst".into(), role: "manager".into() };
                for i in 0..25 {
                    let doc = vec![
                        ("title".to_string(), Value::Text("wo007 burst".into())),
                        ("status".to_string(), Value::Text("Draft".into())),
                        ("month".to_string(), Value::Text("2026-03".into())),
                        ("posting_date".to_string(), Value::Datetime("2026-03-15T00:00:00Z".into())),
                        ("total".to_string(), Value::Float((t * 25 + i) as f64)),
                    ];
                    b.db_write(&caller, &HookChain::default(), WriteOp::Create, "sales_invoice", None, &doc)
                        .expect("1m burst submit");
                }
            })
        })
        .collect();
    for h in handles {
        h.join().expect("burst thread");
    }
    let t = std::time::Instant::now();
    reconcile(
        &db,
        "SELECT month AS k, count() AS n, math::sum(total) AS revenue FROM sales_invoice \
         WHERE month != NONE GROUP BY k TIMEOUT 590s;",
        "si_monthly_1m",
        "revenue",
        "tier1-1m",
    );
    println!("full-scan reconciliation at 1M: exact ({} ms for the live scan)", t.elapsed().as_millis());
}

