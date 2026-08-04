//! Contract -> SurrealQL compilation. The ONLY place in the kernel where
//! query text is assembled. Every dynamic value goes through validate-then-
//! embed rendering here; nothing else may concatenate SurrealQL.
//!
//! (Swap target: the WS RPC `query`+vars form removes embedding entirely;
//! rendering stays behind this module either way.)

use crate::contract::*;

pub const MAX_PATH_DEPTH: usize = 3;

/// Escape a string for a single-quoted SurrealQL literal.
pub fn escape_str(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\'', "\\'")
}

fn is_ident(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

pub fn render_value(v: &Value) -> Result<String, BrokerError> {
    let bad = |detail: &str| BrokerError::InvalidValue { detail: detail.to_string() };
    Ok(match v {
        Value::Null => "NONE".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Int(i) => i.to_string(),
        Value::Float(x) => {
            if !x.is_finite() {
                return Err(bad("non-finite float"));
            }
            format!("{x}f")
        }
        Value::Decimal(s) => {
            let ok = {
                let t = s.strip_prefix('-').unwrap_or(s);
                let parts: Vec<&str> = t.split('.').collect();
                (1..=2).contains(&parts.len())
                    && parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
            };
            if !ok {
                return Err(bad("decimal must be a plain numeric string"));
            }
            format!("{s}dec")
        }
        Value::Datetime(s) => {
            // RFC3339 shape check: 2026-07-24T12:00:00(.fff)?(Z|+hh:mm)
            let shape = s.len() >= 20
                && s.as_bytes()[4] == b'-'
                && s.as_bytes()[7] == b'-'
                && (s.as_bytes()[10] == b'T' || s.as_bytes()[10] == b' ')
                && s.chars().all(|c| c.is_ascii_digit() || "-T:.Z+ ".contains(c));
            if !shape {
                return Err(bad("datetime must be RFC3339"));
            }
            format!("d'{}'", escape_str(s))
        }
        Value::Duration(s) => {
            let ok = !s.is_empty()
                && s.chars().all(|c| c.is_ascii_digit() || "nsumhdwy".contains(c));
            if !ok {
                return Err(bad("bad duration"));
            }
            s.clone()
        }
        Value::RecordId(s) => {
            let (tb, key) = s.split_once(':').ok_or_else(|| bad("record id needs table:key"))?;
            if !is_ident(tb) || key.is_empty() {
                return Err(bad("bad record id"));
            }
            format!("type::record('{}')", escape_str(s))
        }
        Value::Text(s) => format!("'{}'", escape_str(s)),
        Value::List(items) => {
            let parts: Result<Vec<_>, _> = items.iter().map(render_value).collect();
            format!("[{}]", parts?.join(", "))
        }
        Value::Object(entries) => {
            let mut parts = Vec::new();
            for (k, v) in entries {
                if !is_ident(k) {
                    return Err(bad("object keys must be identifiers"));
                }
                parts.push(format!("{k}: {}", render_value(v)?));
            }
            format!("{{ {} }}", parts.join(", "))
        }
    })
}

pub fn render_path(path: &[PathSegment]) -> Result<String, BrokerError> {
    if path.is_empty() || path.len() > MAX_PATH_DEPTH {
        return Err(BrokerError::PathTooDeep { max: MAX_PATH_DEPTH });
    }
    let mut out = String::new();
    for (i, seg) in path.iter().enumerate() {
        match seg {
            PathSegment::Field(name) | PathSegment::LinkHop(name) => {
                if !is_ident(name) {
                    return Err(BrokerError::InvalidValue { detail: format!("bad field name: {name}") });
                }
                if i > 0 {
                    out.push('.');
                }
                out.push_str(name);
            }
            PathSegment::Edge { direction, edge_type } => {
                if !is_ident(edge_type) {
                    return Err(BrokerError::InvalidValue { detail: "bad edge type".into() });
                }
                match direction {
                    EdgeDirection::Out => out.push_str(&format!("->{edge_type}->?")),
                    EdgeDirection::In => out.push_str(&format!("<-{edge_type}<-?")),
                }
            }
        }
    }
    Ok(out)
}

pub fn render_filter(f: &Filter) -> Result<String, BrokerError> {
    Ok(match f {
        Filter::And(fs) => {
            let parts: Result<Vec<_>, _> = fs.iter().map(render_filter).collect();
            format!("({})", parts?.join(" AND "))
        }
        Filter::Or(fs) => {
            let parts: Result<Vec<_>, _> = fs.iter().map(render_filter).collect();
            format!("({})", parts?.join(" OR "))
        }
        Filter::Not(inner) => format!("(!({}))", render_filter(inner)?),
        Filter::Cmp { path, op, value } => {
            let op_s = match op {
                CmpOp::Eq => "=",
                CmpOp::Ne => "!=",
                CmpOp::Gt => ">",
                CmpOp::Gte => ">=",
                CmpOp::Lt => "<",
                CmpOp::Lte => "<=",
                CmpOp::Inside => "INSIDE",
                CmpOp::Contains => "CONTAINS",
            };
            format!("{} {} {}", render_path(path)?, op_s, render_value(value)?)
        }
    })
}

/// `LIVE SELECT` for a subscription. Query text lives here like every other
/// statement the kernel sends — the realtime module composes the subscription,
/// it does not author SurrealQL.
pub fn render_live_select(table: &str) -> Result<String, BrokerError> {
    if !is_ident(table) {
        return Err(BrokerError::InvalidValue { detail: format!("bad table: {table}") });
    }
    Ok(format!("LIVE SELECT * FROM {table};"))
}

pub fn filter_has_range(f: &Filter) -> bool {
    match f {
        Filter::And(fs) | Filter::Or(fs) => fs.iter().any(filter_has_range),
        Filter::Not(inner) => filter_has_range(inner),
        Filter::Cmp { op, .. } => op.is_range(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_is_escaped() {
        let v = Value::Text("a'; REMOVE TABLE x; --".into());
        assert_eq!(render_value(&v).unwrap(), "'a\\'; REMOVE TABLE x; --'");
    }

    #[test]
    fn decimal_validated_and_suffixed() {
        assert_eq!(render_value(&Value::Decimal("123.45".into())).unwrap(), "123.45dec");
        assert!(render_value(&Value::Decimal("123.45; DROP".into())).is_err());
    }

    #[test]
    fn record_id_goes_through_type_record() {
        assert_eq!(
            render_value(&Value::RecordId("app_user:clerk1".into())).unwrap(),
            "type::record('app_user:clerk1')"
        );
        assert!(render_value(&Value::RecordId("no-colon".into())).is_err());
    }

    #[test]
    fn path_depth_capped() {
        let too_deep: Vec<PathSegment> =
            (0..4).map(|i| PathSegment::Field(format!("f{i}"))).collect();
        assert!(matches!(render_path(&too_deep), Err(BrokerError::PathTooDeep { .. })));
    }

    #[test]
    fn filter_compiles() {
        let f = Filter::And(vec![
            Filter::Cmp {
                path: vec![PathSegment::Field("status".into())],
                op: CmpOp::Eq,
                value: Value::Text("Draft".into()),
            },
            Filter::Cmp {
                path: vec![PathSegment::Field("total".into())],
                op: CmpOp::Gte,
                value: Value::Int(100),
            },
        ]);
        assert_eq!(render_filter(&f).unwrap(), "(status = 'Draft' AND total >= 100)");
        assert!(filter_has_range(&f));
    }

    #[test]
    fn bad_field_names_rejected() {
        let f = Filter::Cmp {
            path: vec![PathSegment::Field("x; REMOVE TABLE y".into())],
            op: CmpOp::Eq,
            value: Value::Int(1),
        };
        assert!(render_filter(&f).is_err());
    }
}
