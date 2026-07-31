//! WO-012: the writer-tax curve through the KERNEL's own subscriptions.
//! Sets the live-subscription budget from measurement instead of the spike's
//! client-side estimate. Run explicitly:
//!
//!   cargo test --release --test live_tax_curve -- --ignored --nocapture

use std::sync::Arc;
use std::time::Instant;

use frust_kernel::broker::{Broker, Caller, HookChain, HookDispatch};
use frust_kernel::contract::*;
use frust_kernel::db::scoped_db;
use frust_kernel::tenancy::single_tenant;
use frust_kernel::meta::{access_ddl, identity_ddl};
use frust_kernel::realtime::Realtime;

struct PassHooks;
impl HookDispatch for PassHooks {
    fn validate(&self, doctype: &str, doc: &[(String, Value)]) -> Result<Vec<(String, Value)>, BrokerError> {
        Ok(doc.to_vec())
    }
}

fn median(mut v: Vec<u128>) -> u128 {
    v.sort_unstable();
    v[v.len() / 2]
}

#[test]
#[ignore = "measurement run: sets the budget; needs a fresh instance"]
fn writer_tax_per_parked_subscription() {
    let cfg = single_tenant(&"rt_tax".to_string()).expect("tenancy");
    let db = scoped_db(&cfg);
    db.sql_root_ns("REMOVE DATABASE IF EXISTS rt_tax; DEFINE DATABASE rt_tax;").unwrap();
    db.sql_root(&format!(
        "{}; {}; \
         CREATE app_user:u SET name = 'u', role = 'manager', pass = crypto::argon2::generate('pw-u'); \
         DEFINE TABLE thing SCHEMALESS PERMISSIONS FULL; \
         DEFINE FIELD owner ON thing TYPE option<record<app_user>> DEFAULT $auth.id; \
         DEFINE TABLE doctype SCHEMALESS PERMISSIONS NONE; \
         CREATE doctype CONTENT {{ name: 'thing', label: 'Thing', fields: [] }};",
        identity_ddl(),
        access_ddl(),
    ))
    .unwrap();

    let b = Arc::new(Broker::new(scoped_db(&cfg), Box::new(PassHooks)));
    let caller = Caller { user: "u".into(), pass: "pw-u".into(), role: "manager".into() };
    let doc = vec![
        ("title".to_string(), Value::Text("tax".into())),
        ("status".to_string(), Value::Text("Draft".into())),
    ];
    let realtime = Realtime::new(cfg.endpoint());
    let jwt = db.signin("u", "pw-u").unwrap();

    let sample = |n: usize| -> u128 {
        for _ in 0..20 {
            b.db_write(&caller, &HookChain::default(), WriteOp::Create, "thing", None, &doc).unwrap();
        }
        let mut s = Vec::new();
        for _ in 0..40 {
            let t = Instant::now();
            b.db_write(&caller, &HookChain::default(), WriteOp::Create, "thing", None, &doc).unwrap();
            s.push(t.elapsed().as_millis());
        }
        let m = median(s);
        println!("parked={n:<4} submit median={m} ms");
        m
    };

    println!("=== writer tax curve (kernel-side subscriptions) ===");
    let base = sample(0);
    let mut subs = Vec::new();
    let mut curve = vec![(0usize, base)];
    // stop at the enforced budget: subscribe() refuses beyond it by design
    for target in [5usize, 10, 15, 20, 30] {
        while subs.len() < target {
            subs.push(realtime.subscribe("u", &jwt, &cfg, "thing").expect("subscribe"));
        }
        curve.push((target, sample(target)));
    }
    for s in &subs {
        let _ = realtime.unsubscribe(s, "u");
    }
    // bracket: re-measure with ZERO subs at the end. If this differs from the
    // opening baseline, the machine drifted during the run and the curve's
    // deltas are drift, not tax (the substrate caveat, applied inside a test).
    std::thread::sleep(std::time::Duration::from_millis(500));
    let base_after = sample(0);

    println!("\n=== tax model ===");
    for (n, m) in &curve {
        let delta = *m as i128 - base as i128;
        let per = if *n > 0 { delta as f64 * 1000.0 / *n as f64 } else { 0.0 };
        println!("parked={n:<4} median={m} ms  delta={delta:+} ms  per-sub={per:.0} us");
    }
    println!(
        "\nbaseline open {base} ms / close {base_after} ms (drift {:+} ms); floor 25 ms",
        base_after as i128 - base as i128
    );
}

