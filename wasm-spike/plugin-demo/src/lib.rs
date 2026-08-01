//! Demo compiled plugin, v2: the dynamic-doc envelope (ADR-006 edge 3).
//! Same business rules as the spike original â€” negative totals reject,
//! large drafts get flagged â€” now over the full document.

wit_bindgen::generate!({ path: "../../frust-kernel/wit", world: "route-plugin" });

use exports::frust::plugin::hooks::{Entry, Guest, Value};
use exports::frust::plugin::routes::{Guest as RouteGuest, Request, Response};
use frust::plugin::db_api::db_read;
use frust::plugin::host_api::log;

fn get<'a>(doc: &'a [Entry], key: &str) -> Option<&'a Value> {
    doc.iter().find(|e| e.key == key).map(|e| &e.val)
}

fn set(doc: &mut Vec<Entry>, key: &str, val: Value) {
    if let Some(e) = doc.iter_mut().find(|e| e.key == key) {
        e.val = val;
    } else {
        doc.push(Entry { key: key.to_string(), val });
    }
}

fn as_number(v: &Value) -> Option<f64> {
    match v {
        Value::FloatV(x) => Some(*x),
        Value::IntV(i) => Some(*i as f64),
        Value::DecimalV(s) => s.parse().ok(),
        _ => None,
    }
}

struct Plugin;

impl Guest for Plugin {
    fn validate(doc: Vec<Entry>) -> Result<Vec<Entry>, String> {
        let mut doc = doc;
        let total = get(&doc, "total").and_then(as_number).unwrap_or(0.0);
        if total < 0.0 {
            return Err(format!("total must not be negative (got {total})"));
        }
        let status = match get(&doc, "status") {
            Some(Value::TextV(s)) => s.clone(),
            _ => String::new(),
        };
        if status == "Draft" && total > 10_000.0 {
            let id = match get(&doc, "id") {
                Some(Value::RecordIdV(s) | Value::TextV(s)) => s.clone(),
                _ => "?".to_string(),
            };
            log(&format!("large draft flagged: {id}"));
            set(&mut doc, "status", Value::TextV("Needs Approval".to_string()));
        }
        Ok(doc)
    }

    fn spin() {
        loop {
            std::hint::black_box(0u64);
        }
    }

    fn hog() {
        let mut v: Vec<Vec<u8>> = Vec::new();
        loop {
            v.push(vec![0xA5; 1024 * 1024]);
            std::hint::black_box(&v);
        }
    }
}

/// WO-019 criterion 1: the probe route.
///
/// `?probe=` selects a hostile attempt so the escapes are exercised by a real
/// guest through the real boundary, not simulated host-side.
impl RouteGuest for Plugin {
    fn handle(req: Request) -> Response {
        let probe = req
            .query
            .split('&')
            .find_map(|kv| kv.strip_prefix("probe="))
            .unwrap_or("read");
        // WO-019 criterion 7: the honest read takes its doctype from the
        // request, so a demo app's route can read the app's OWN data instead
        // of a name hardcoded during the criterion-1 probe. The hostile arms
        // below keep their fixed targets deliberately — they are testing the
        // boundary, not serving anyone.
        let doctype = req
            .query
            .split('&')
            .find_map(|kv| kv.strip_prefix("doctype="))
            .unwrap_or("purchase_order");

        let body = match probe {
            // ---- the honest path: a structured read through the door ----
            // No projection: the caller's readable set decides. Naming fields
            // here would tie this handler to one app's schema, and asking for
            // a field the doctype lacks is refused (E_FIELD_NOT_READABLE) —
            // correctly, but that is the envelope's job, not this route's.
            "read" => match db_read(doctype, "null", &[]) {
                Ok(rows) => format!("ok:{rows}"),
                Err(e) => format!("denied:{e}"),
            },

            // ---- hostile 1: raw SurrealQL smuggled as the doctype ----
            "raw-doctype" => {
                match db_read("purchase_order; REMOVE TABLE purchase_order", "null", &[]) {
                    Ok(rows) => format!("ESCAPED:{rows}"),
                    Err(e) => format!("refused:{e}"),
                }
            }

            // ---- hostile 2: raw SurrealQL smuggled through the filter ----
            "raw-filter" => match db_read(
                "purchase_order",
                "\"1=1; SELECT * FROM app_user\"",
                &[],
            ) {
                Ok(rows) => format!("ESCAPED:{rows}"),
                Err(e) => format!("refused:{e}"),
            },

            // ---- hostile 3: read a table this caller must not see ----
            "root-leak" => match db_read("app_user", "null", &["name".into(), "pass".into()]) {
                Ok(rows) => format!("ESCAPED:{rows}"),
                Err(e) => format!("refused:{e}"),
            },

            // ---- hostile 4: is there any handle/socket in this world? ----
            // If a plugin can open its own connection, the door is irrelevant.
            "connect" => {
                match std::net::TcpStream::connect("127.0.0.1:8899") {
                    Ok(_) => "ESCAPED:socket opened".to_string(),
                    Err(e) => format!("refused:{e}"),
                }
            }

            // ---- hostile 5: reach the filesystem for credentials ----
            "fs" => match std::fs::read_to_string("/etc/passwd")
                .or_else(|_| std::fs::read_to_string("C:/Windows/win.ini"))
            {
                Ok(s) => format!("ESCAPED:{} bytes", s.len()),
                Err(e) => format!("refused:{e}"),
            },

            other => format!("unknown probe:{other}"),
        };

        Response { status: 200, body }
    }
}

export!(Plugin);

