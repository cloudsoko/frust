//! Tier-2 script engine, v2: dynamic-doc envelope (ADR-006 edge 3).
//! The user script sees a natural JS `doc` object; the Rust shell owns the
//! type discipline — each field's WIT variant kind is recorded on the way in
//! and re-imposed on the way out, so a JS script cannot silently turn a
//! decimal into a float: decimals cross the boundary as strings both ways.

wit_bindgen::generate!({ path: "../../frust-kernel/wit", world: "plugin" });

use std::cell::RefCell;
use std::collections::HashMap;

use boa_engine::{
    js_string,
    object::ObjectInitializer,
    property::Attribute,
    Context, JsError, JsNativeError, JsResult, JsValue, NativeFunction, Source,
};

use exports::frust::plugin::hooks::{Entry, Guest, Value};
use frust::plugin::host_api::log;

/// WO-030: the SAME `decimal.rs` the kernel and DB reconcile against, compiled
/// into this guest — not a JS reimplementation. `include!` (not a shared crate)
/// because the file is a dependency-free leaf and this crate already reaches
/// into `../../frust-kernel/` for its WIT; one source, two compile targets, so
/// three hosts cannot drift (WO-021's byte-equal property, extended). The
/// binding adds NO host capability: it is pure arithmetic inside the sandbox,
/// not a new WIT import, so the WO-017 containment posture is unchanged.
#[allow(dead_code)] // reused verbatim; the guest uses a subset (no from_json etc.)
mod decimal {
    include!(concat!(env!("OUT_DIR"), "/decimal.rs"));
}

const DEFAULT_SCRIPT: &str = include_str!("../scripts/validate.js");

/// A native `Decimal.<op>` throw: a typed money error the user script sees as a
/// JS exception (which becomes the reject), never a silent wrong number.
fn money_throw(msg: String) -> JsError {
    JsNativeError::error().with_message(msg).into()
}

/// Read arg `i` as a string and parse it as an exact decimal, or throw. Every
/// op constructs from strings — money crosses this boundary as a string both
/// ways, same as it crosses the WIT and DB boundaries.
fn arg_dec(args: &[JsValue], i: usize, ctx: &mut Context) -> JsResult<decimal::Decimal> {
    let s = args.get(i).cloned().unwrap_or_default().to_string(ctx)?.to_std_string_lossy();
    decimal::Decimal::parse(&s).ok_or_else(|| {
        money_throw(format!(
            "Decimal.parse: \"{s}\" is not a plain decimal number (float notation like 1e5 is \
             refused — money never travels as a float). [FRUST:E_MONEY_NOT_NUMERIC]"
        ))
    })
}

/// Read arg `i` as a non-negative integer scale (fractional digit count).
fn arg_scale(args: &[JsValue], i: usize, ctx: &mut Context) -> JsResult<u32> {
    let n = args.get(i).cloned().unwrap_or_default().to_number(ctx)?;
    if !n.is_finite() || n < 0.0 || n.fract() != 0.0 || n > 38.0 {
        return Err(money_throw(format!(
            "scale must be an integer in 0..=38, got {n} [FRUST:E_MONEY_SCALE]"
        )));
    }
    Ok(n as u32)
}

/// Read an optional rounding mode arg; default half-even (the accounting
/// default, REQ-6.2.2). Explicit per call — there is no global rounding config.
fn arg_mode(args: &[JsValue], i: usize, ctx: &mut Context) -> JsResult<decimal::Mode> {
    let raw = match args.get(i) {
        None => return Ok(decimal::Mode::HalfEven),
        Some(v) if v.is_undefined() || v.is_null() => return Ok(decimal::Mode::HalfEven),
        Some(v) => v.clone().to_string(ctx)?.to_std_string_lossy(),
    };
    match raw.as_str() {
        "half_even" | "" => Ok(decimal::Mode::HalfEven),
        "half_up" => Ok(decimal::Mode::HalfUp),
        "down" => Ok(decimal::Mode::Down),
        other => Err(money_throw(format!(
            "unknown rounding mode \"{other}\" (use \"half_even\", \"half_up\", or \"down\") \
             [FRUST:E_MONEY_MODE]"
        ))),
    }
}

fn money_err(e: decimal::MoneyError) -> JsError {
    money_throw(match e {
        decimal::MoneyError::Overflow => "money arithmetic overflowed i128 range [FRUST:E_MONEY_OVERFLOW]".into(),
        decimal::MoneyError::DivByZero => "division by zero [FRUST:E_MONEY_DIVBYZERO]".into(),
    })
}

fn dec_result(d: decimal::Decimal) -> JsResult<JsValue> {
    Ok(js_string!(d.to_plain_string().as_str()).into())
}

/// Register the `Decimal` namespace: exact string-in / string-out money math,
/// backed verbatim by the kernel's `decimal.rs`.
///
///   Decimal.add(a, b)            Decimal.sub(a, b)
///   Decimal.mul(a, b)            // EXACT, scale grows — round explicitly
///   Decimal.div(a, b, scale, mode)   Decimal.round(a, scale, mode)
///   Decimal.cmp(a, b) -> -1 | 0 | 1  // numeric, so "1.50" == "1.5"
///
/// Rounding is NEVER implicit (REQ-6.2.2): `mul` grows the scale and the author
/// must `round` at a defined point; `div`/`round` take scale + mode explicitly.
fn register_decimal(ctx: &mut Context) {
    let obj = ObjectInitializer::new(ctx)
        .function(
            NativeFunction::from_fn_ptr(|_t, a, c| dec_result(arg_dec(a, 0, c)?.add(arg_dec(a, 1, c)?))),
            js_string!("add"),
            2,
        )
        .function(
            NativeFunction::from_fn_ptr(|_t, a, c| {
                dec_result(arg_dec(a, 0, c)?.add(arg_dec(a, 1, c)?.neg()))
            }),
            js_string!("sub"),
            2,
        )
        .function(
            NativeFunction::from_fn_ptr(|_t, a, c| {
                arg_dec(a, 0, c)?.mul(arg_dec(a, 1, c)?).map_err(money_err).and_then(dec_result)
            }),
            js_string!("mul"),
            2,
        )
        .function(
            NativeFunction::from_fn_ptr(|_t, a, c| {
                let (x, y) = (arg_dec(a, 0, c)?, arg_dec(a, 1, c)?);
                let (scale, mode) = (arg_scale(a, 2, c)?, arg_mode(a, 3, c)?);
                x.div_round(y, scale, mode).map_err(money_err).and_then(dec_result)
            }),
            js_string!("div"),
            4,
        )
        .function(
            NativeFunction::from_fn_ptr(|_t, a, c| {
                let x = arg_dec(a, 0, c)?;
                let (scale, mode) = (arg_scale(a, 1, c)?, arg_mode(a, 2, c)?);
                dec_result(x.round(scale, mode))
            }),
            js_string!("round"),
            3,
        )
        .function(
            NativeFunction::from_fn_ptr(|_t, a, c| {
                let (x, y) = (arg_dec(a, 0, c)?, arg_dec(a, 1, c)?);
                Ok(JsValue::from(match x.cmp(&y) {
                    std::cmp::Ordering::Less => -1,
                    std::cmp::Ordering::Equal => 0,
                    std::cmp::Ordering::Greater => 1,
                }))
            }),
            js_string!("cmp"),
            2,
        )
        .build();
    ctx.register_global_property(js_string!("Decimal"), obj, Attribute::all())
        .expect("register Decimal");
}

struct Engine {
    ctx: Context,
    prelude: boa_engine::Script,
    script: boa_engine::Script,
    epilogue: boa_engine::Script,
}

thread_local! {
    static ENGINE: RefCell<Option<Engine>> = const { RefCell::new(None) };
}

fn make_engine(src: &str) -> Engine {
    let mut ctx = Context::default();
    let log_fn = NativeFunction::from_fn_ptr(|_this, args, ctx| {
        let msg = args
            .first()
            .cloned()
            .unwrap_or_default()
            .to_string(ctx)?
            .to_std_string_lossy();
        log(&msg);
        Ok(JsValue::undefined())
    });
    ctx.register_global_callable(js_string!("log"), 1, log_fn).expect("register log");
    register_decimal(&mut ctx);

    let parse = |ctx: &mut Context, s: &str| {
        boa_engine::Script::parse(Source::from_bytes(s.as_bytes()), None, ctx).expect("parses")
    };
    let prelude = parse(&mut ctx, "var doc = JSON.parse(__doc_json); doc;");
    let script = parse(&mut ctx, src);
    // The NaN catch has to run BEFORE stringify, because `JSON.stringify`
    // turns NaN and Infinity into `null` — which is indistinguishable from a
    // script deliberately clearing a field. Catching it here keeps "cleared"
    // legal and "corrupted" loud; one step later, both look identical.
    let epilogue = parse(
        &mut ctx,
        r#"(function () {
            for (var k in doc) {
                var v = doc[k];
                if (typeof v === "number" && !isFinite(v)) {
                    throw new Error("Field '" + k + "' was set to " + String(v)
                        + ", which is not a number. [FRUST:E_FIELD_NAN]");
                }
            }
            return JSON.stringify(doc);
        })()"#,
    );
    Engine { ctx, prelude, script, epilogue }
}

fn with_engine<R>(f: impl FnOnce(&mut Engine) -> R) -> R {
    ENGINE.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_none() {
            let src = std::env::var("FRUST_SCRIPT").unwrap_or_else(|_| DEFAULT_SCRIPT.to_string());
            *slot = Some(make_engine(&src));
        }
        f(slot.as_mut().unwrap())
    })
}

/// WIT value -> the JSON the script sees. Decimal stays a string.
fn to_json(v: &Value) -> serde_json::Value {
    match v {
        Value::NullV => serde_json::Value::Null,
        Value::BoolV(b) => serde_json::json!(b),
        Value::IntV(i) => serde_json::json!(i),
        Value::FloatV(x) => serde_json::json!(x),
        Value::DecimalV(s) | Value::TextV(s) | Value::DatetimeV(s) | Value::DurationV(s)
        | Value::RecordIdV(s) => serde_json::json!(s),
        Value::CompoundV(raw) => serde_json::from_str(raw).unwrap_or(serde_json::Value::Null),
    }
}

/// JSON coming back from the script -> WIT value, re-imposing the INPUT
/// kind where the field existed (type discipline lives here, not in JS).
///
/// WO-017 item 3: for a decimal field this REFUSES rather than coerces. The
/// shell was already the place that re-imposes kinds; the catch is simply that
/// shell declining to re-impose what a script corrupted. Because it lives in
/// the shared artifact, one rebuild protects the kernel and the browser alike.
fn from_json(v: &serde_json::Value, input_kind: Option<&Value>, key: &str) -> Result<Value, String> {
    Ok(match input_kind {
        Some(Value::DecimalV(_)) => return decimal_out(v, key),
        Some(Value::DatetimeV(_)) if v.is_string() => Value::DatetimeV(v.as_str().unwrap().to_string()),
        Some(Value::DurationV(_)) if v.is_string() => Value::DurationV(v.as_str().unwrap().to_string()),
        Some(Value::RecordIdV(_)) if v.is_string() => Value::RecordIdV(v.as_str().unwrap().to_string()),
        Some(Value::IntV(_)) if v.is_i64() => Value::IntV(v.as_i64().unwrap()),
        Some(Value::FloatV(_)) if v.is_number() => Value::FloatV(v.as_f64().unwrap()),
        _ => infer(v),
    })
}

/// REQ-6.2.1 at the script boundary: a decimal field may leave a script as an
/// exact decimal STRING, as an integral number, as an explicit null, or not at
/// all. It may never leave as a fractional number, because a fractional JS
/// number IS a float and carries float error — `0.1 + 0.2` returns
/// `0.30000000000000004`, and storing that would be the very defect the
/// requirement forbids.
///
/// Integral numbers are allowed because they are exactly representable and
/// round-trip to an exact decimal: there is no error to catch. Fractional ones
/// are refused even when they happen to be exact (`10.5` is; `10.1` is not),
/// because a rule an author can apply without knowing which is which is worth
/// more than a rule that is usually right.
///
/// THE DECIMAL-SAFE PATH, for script authors:
///   - money ARRIVES as a string — never do bare arithmetic on it
///     (`doc.total + 1` concatenates)
///   - to derive money: `Number()` it, round explicitly, write it back with
///     `.toFixed(n)` — a string, which this function re-imposes exactly
///   - exact money arithmetic belongs on the server (REQ-6.2.2), not in a
///     script; a script should route, flag and label money, not compute it
fn decimal_out(v: &serde_json::Value, key: &str) -> Result<Value, String> {
    match v {
        serde_json::Value::String(s) if is_decimal_literal(s) => Ok(Value::DecimalV(s.clone())),
        serde_json::Value::String(s) => Err(format!(
            "Currency field '{key}' was set to \"{s}\", which is not a number. [FRUST:E_MONEY_NOT_NUMERIC]"
        )),
        serde_json::Value::Number(n) => match n.as_i64() {
            Some(i) => Ok(Value::DecimalV(i.to_string())),
            None => Err(format!(
                "Currency field '{key}' was computed as a floating-point number ({n}). \
                 Money must stay exact — round it and write it back as a string. [FRUST:E_MONEY_FLOAT]"
            )),
        },
        // An explicit clear stays legal. NaN cannot reach here disguised as
        // null: the epilogue rejects non-finite numbers BEFORE JSON.stringify
        // erases the difference between them.
        serde_json::Value::Null => Ok(Value::NullV),
        serde_json::Value::Bool(_) => Err(format!(
            "Currency field '{key}' was replaced with a true/false value. [FRUST:E_MONEY_TYPE]"
        )),
        _ => Err(format!(
            "Currency field '{key}' was replaced with a list or object. [FRUST:E_MONEY_TYPE]"
        )),
    }
}

/// A plain decimal literal: optional sign, digits, at most one point.
/// Deliberately rejects exponent form (`1e3`) — that is float notation, and
/// the point of this path is that money never travels as a float.
fn is_decimal_literal(s: &str) -> bool {
    let t = s.strip_prefix('-').or_else(|| s.strip_prefix('+')).unwrap_or(s);
    if t.is_empty() {
        return false;
    }
    let mut seen_dot = false;
    let mut digits = 0usize;
    for c in t.chars() {
        match c {
            '.' if !seen_dot => seen_dot = true,
            '0'..='9' => digits += 1,
            _ => return false,
        }
    }
    digits > 0
}

fn infer(v: &serde_json::Value) -> Value {
    match v {
        serde_json::Value::Null => Value::NullV,
        serde_json::Value::Bool(b) => Value::BoolV(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::IntV(i)
            } else {
                Value::FloatV(n.as_f64().unwrap_or(0.0))
            }
        }
        serde_json::Value::String(s) => Value::TextV(s.clone()),
        other => Value::CompoundV(other.to_string()),
    }
}

fn run_validate(engine: &mut Engine, doc: Vec<Entry>) -> Result<Vec<Entry>, String> {
    let kinds: HashMap<String, Value> =
        doc.iter().map(|e| (e.key.clone(), e.val.clone())).collect();
    let json_in = serde_json::Value::Object(
        doc.iter().map(|e| (e.key.clone(), to_json(&e.val))).collect(),
    )
    .to_string();

    let ctx = &mut engine.ctx;
    ctx.global_object()
        .set(js_string!("__doc_json"), js_string!(json_in.as_str()), false, ctx)
        .map_err(|e| format!("set doc: {e}"))?;
    engine.prelude.evaluate(ctx).map_err(|e| e.to_string())?;
    // the user script: a JS throw is the reject path
    engine.script.evaluate(ctx).map_err(|e| strip_trace(&e.to_string()))?;
    // The epilogue can now reject (the NaN catch), so its throw is a
    // user-facing message and gets the same trace stripping as the script's.
    let out = engine
        .epilogue
        .evaluate(ctx)
        .map_err(|e| strip_trace(&e.to_string()))?
        .to_string(ctx)
        .map_err(|e| e.to_string())?
        .to_std_string_lossy();

    let parsed: serde_json::Value =
        serde_json::from_str(&out).map_err(|e| format!("script produced non-JSON doc: {e}"))?;
    let serde_json::Value::Object(map) = parsed else {
        return Err("script destroyed doc".to_string());
    };
    let mut out_doc = Vec::with_capacity(map.len());
    for (k, v) in map {
        let val = from_json(&v, kinds.get(&k), &k)?;
        out_doc.push(Entry { key: k, val });
    }
    Ok(out_doc)
}

/// ADR-007 hygiene: user-facing rejects never carry engine trace suffixes.
///
/// Two shapes to remove, not one. Dropping trailing lines is not enough —
/// Boa also appends a source position to the FIRST line, e.g.
/// `Error: needs a visa (unknown at :2:9)`. A line/column inside a script the
/// user cannot see is engine internals by any reading, so it goes too.
fn strip_trace(e: &str) -> String {
    let first = e.lines().next().unwrap_or(e).trim().trim_matches('"');
    // Only strip a suffix that really is a position: `(… at <line>:<col>)`.
    // A message legitimately ending in parentheses must survive intact.
    if let Some(open) = first.rfind(" (") {
        let inner = &first[open + 2..];
        if let Some(pos) = inner.strip_suffix(')').and_then(|i| i.rsplit(" at ").next()) {
            let mut parts = pos.rsplitn(3, ':');
            let col = parts.next().unwrap_or("");
            let line = parts.next().unwrap_or("");
            let numeric = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
            if numeric(col) && numeric(line) {
                return first[..open].trim().to_string();
            }
        }
    }
    first.to_string()
}

struct ScriptEngine;

impl Guest for ScriptEngine {
    fn validate(doc: Vec<Entry>) -> Result<Vec<Entry>, String> {
        with_engine(|engine| run_validate(engine, doc))
    }

    fn spin() {
        with_engine(|engine| {
            let _ = engine.ctx.eval(Source::from_bytes(b"while (true) {}" as &[u8]));
        });
    }

    fn hog() {
        with_engine(|engine| {
            let _ = engine.ctx.eval(Source::from_bytes(
                b"var a = []; while (true) { a.push(new Array(65536).fill(1)); }" as &[u8],
            ));
        });
    }
}

export!(ScriptEngine);
