//! Kernel-owned live subscriptions.
//!
//! A subscription is a per-user `LIVE SELECT` held BY THE KERNEL over a WS
//! RPC connection authenticated with the subscriber's own record JWT — the
//! DB enforces the row clause on the push path (REQ-6.5.1); the kernel never
//! filters it. Notifications are buffered as INVALIDATION TICKS (action +
//! record id, no row data): clients refetch through the normal read door, so
//! the push path carries nothing a leak could ride on.
//!
//! The measured limits: parked LIVEs are free at idle, but every
//! write pays ~70 µs per subscription on its table — so subscriptions are
//! BUDGETED per table, and idle ones (no poll for IDLE_REAP) are reaped:
//! parked count tracks focused views, not open tabs. Over budget is a loud
//! typed refusal; the client stays on polling (REQ-6.5.2, transparent).

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::contract::BrokerError;
use crate::telemetry;
use crate::wsrpc::WsRpc;

/// The measured budget. Early estimates put it at ~70 µs/sub client-side; the
/// kernel-side curve (`live_tax_curve`, bracketed against drift) confirms
/// ~50-100 µs/sub: +1 ms at 5-20 parked subs, +2 ms at 30, +4 ms at 40.
/// 20 is the largest budget whose tax stays inside `LIVE_TAX_BUDGET_MS`.
/// Raising it requires the gate to stay green — never the gate to widen.
///
/// This figure has been examined and left alone: the gate that would justify
/// changing it was measuring an order effect, not subscription cost —
/// lowering the budget to 12 made the measured "tax" go UP, which is not a
/// thing a real per-subscription cost can do.
pub const LIVE_SUB_BUDGET_PER_TABLE: usize = 20;

/// How much of the writer's latency realtime may cost at full budget. CI
/// asserts it (`gate_submit_with_live_subscriptions`). This is a SEPARATE
/// allowance from REQ-6.1.1's 25 ms floor, deliberately: the floor is a
/// contract about the write path, and quietly widening it to absorb an
/// optional feature would be a spec change by stealth.
pub const LIVE_TAX_BUDGET_MS: u128 = 2;

/// A subscription with no /events poll for this long has lost focus (tab
/// blurred, navigated away, crashed): reap it.
pub const IDLE_REAP: Duration = Duration::from_secs(30);

const TICK_BUFFER_CAP: usize = 512;

struct Subscription {
    user: String,
    table: String,
    tenant: String,
    conn: WsRpc,
    ticks: Arc<Mutex<VecDeque<serde_json::Value>>>,
    last_polled: Mutex<Instant>,
}

pub struct Realtime {
    /// surreal WS host:port (e.g. "127.0.0.1:8899").
    addr: String,
    subs: Mutex<HashMap<String, Arc<Subscription>>>,
}

impl Realtime {
    /// Holds no namespace. Placement arrives per subscription, on a
    /// `ResolvedTenant` nobody can forge — never a ns/db selection on the
    /// shared client, outside the tenancy seam.
    pub fn new(endpoint: &str) -> Self {
        let addr = endpoint.trim_start_matches("http://").trim_start_matches("https://").to_string();
        Self { addr, subs: Mutex::new(HashMap::new()) }
    }

    fn count_for_table(&self, tenant: &str, table: &str) -> usize {
        self.subs
            .lock()
            .unwrap()
            .values()
            .filter(|s| s.table == table && s.tenant == tenant)
            .count()
    }

    fn publish_gauge(&self, tenant: &str, table: &str) {
        telemetry::gauge(
            "frust_live_subscriptions",
            &[("table", table), ("tenant", tenant)],
            self.count_for_table(tenant, table) as f64,
        );
    }

    /// Opens a live subscription for `user` on `table`, authenticated with
    /// THEIR jwt. Refuses loudly over budget (the client stays polling).
    pub fn subscribe(
        &self,
        user: &str,
        jwt: &str,
        target: &crate::tenancy::ResolvedTenant,
        table: &str,
    ) -> Result<String, BrokerError> {
        // the gauge/budget key is the TENANT; the `use` below is the PLACEMENT
        let tenant = target.tenant_id().as_str();
        if !table.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') || table.is_empty() {
            return Err(BrokerError::InvalidValue { detail: format!("bad table: {table}") });
        }
        if self.count_for_table(tenant, table) >= LIVE_SUB_BUDGET_PER_TABLE {
            return Err(BrokerError::Db {
                detail: format!(
                    "FRUST:E_SUB_BUDGET: table {table} at its live budget ({LIVE_SUB_BUDGET_PER_TABLE}); poll instead"
                ),
            });
        }

        let ticks: Arc<Mutex<VecDeque<serde_json::Value>>> = Arc::new(Mutex::new(VecDeque::new()));
        let sink = Arc::clone(&ticks);
        let table_owned = table.to_string();
        let tenant_owned = tenant.to_string();
        let conn = WsRpc::connect(&self.addr, move |msg| {
            // invalidation tick only: action + record id — never the row
            let Some(result) = msg.get("result") else { return };
            let action = result.get("action").and_then(|a| a.as_str()).unwrap_or("");
            if action.is_empty() {
                return;
            }
            let rid = result
                .get("result")
                .and_then(|r| r.get("id"))
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            let mut q = sink.lock().unwrap();
            if q.len() == TICK_BUFFER_CAP {
                q.pop_front(); // overflow: client refetch covers the gap anyway
            }
            q.push_back(serde_json::json!({ "action": action, "id": rid }));
            telemetry::inc(
                "frust_live_ticks_total",
                &[("table", &table_owned), ("tenant", &tenant_owned)],
                1,
            );
        })?;

        // the subscriber's OWN session: the DB filters the live rows
        conn.call("authenticate", serde_json::json!([jwt]))?;
        conn.call(
            "use",
            serde_json::json!([target.namespace().as_str(), target.database().as_str()]),
        )?;
        conn.call("query", serde_json::json!([crate::surql::render_live_select(table)?]))?;

        let id = telemetry::TraceId::new().as_str().to_string();
        let sub = Arc::new(Subscription {
            user: user.to_string(),
            table: table.to_string(),
            tenant: tenant.to_string(),
            conn,
            ticks,
            last_polled: Mutex::new(Instant::now()),
        });
        self.subs.lock().unwrap().insert(id.clone(), sub);
        self.publish_gauge(tenant, table);
        telemetry::emit(
            telemetry::Level::Info,
            "live_subscribe",
            &[
                ("sub", serde_json::json!(id)),
                ("table", serde_json::json!(table)),
                ("user", serde_json::json!(user)),
            ],
        );
        Ok(id)
    }

    /// Drains buffered ticks. `alive = false` tells the client its LIVE died
    /// (surreal restart): reconnect = resubscribe + refetch.
    pub fn drain(&self, sub_id: &str, user: &str) -> Result<(Vec<serde_json::Value>, bool), BrokerError> {
        let sub = self
            .subs
            .lock()
            .unwrap()
            .get(sub_id)
            .cloned()
            .ok_or_else(|| BrokerError::UnknownDoctype { name: format!("subscription {sub_id}") })?;
        if sub.user != user {
            return Err(BrokerError::PermissionDenied { detail: "not your subscription".into() });
        }
        *sub.last_polled.lock().unwrap() = Instant::now();
        let events: Vec<serde_json::Value> = sub.ticks.lock().unwrap().drain(..).collect();
        Ok((events, sub.conn.is_alive()))
    }

    pub fn unsubscribe(&self, sub_id: &str, user: &str) -> Result<(), BrokerError> {
        let sub = {
            let mut subs = self.subs.lock().unwrap();
            match subs.get(sub_id) {
                Some(s) if s.user == user => subs.remove(sub_id),
                Some(_) => return Err(BrokerError::PermissionDenied { detail: "not your subscription".into() }),
                None => None,
            }
        };
        if let Some(s) = sub {
            s.conn.close();
            self.publish_gauge(&s.tenant, &s.table);
        }
        Ok(())
    }

    /// Reaps idle and dead subscriptions; called from the serve tick. Parked
    /// count tracks FOCUSED views: blur -> polls stop -> reaped here.
    pub fn reap(&self) {
        let doomed: Vec<(String, Arc<Subscription>)> = {
            let subs = self.subs.lock().unwrap();
            subs.iter()
                .filter(|(_, s)| {
                    !s.conn.is_alive() || s.last_polled.lock().unwrap().elapsed() > IDLE_REAP
                })
                .map(|(k, s)| (k.clone(), Arc::clone(s)))
                .collect()
        };
        if doomed.is_empty() {
            return;
        }
        let mut subs = self.subs.lock().unwrap();
        for (id, s) in doomed {
            subs.remove(&id);
            s.conn.close();
            drop(s.ticks.lock().unwrap().drain(..));
            telemetry::inc("frust_live_reaped_total", &[("table", &s.table), ("tenant", &s.tenant)], 1);
            telemetry::emit(
                telemetry::Level::Info,
                "live_reaped",
                &[("sub", serde_json::json!(id)), ("table", serde_json::json!(s.table))],
            );
        }
        drop(subs);
        // republish gauges for affected tables
        let pairs: Vec<(String, String)> = {
            let subs = self.subs.lock().unwrap();
            subs.values().map(|s| (s.tenant.clone(), s.table.clone())).collect()
        };
        for (tenant, table) in pairs {
            self.publish_gauge(&tenant, &table);
        }
    }

    pub fn active(&self) -> usize {
        self.subs.lock().unwrap().len()
    }
}
