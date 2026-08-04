//! Guard: no internal work-order (`WO-###`) or architecture-decision
//! (`ADR-###`) reference may appear in a string the kernel can hand back to a
//! caller.
//!
//! The plugin-route surface returns error text to whoever made the request —
//! including unauthenticated callers, before any session exists. An internal
//! citation in that text leaks project internals and means nothing to the
//! person reading it; the message must describe the rule in product language.
//!
//! This is a source scanner in the spirit of `surql_monopoly` /
//! `tenancy_monopoly`: an invariant that lives only in a reviewer's head
//! decays, one that fails the build does not. It is database-free — it reads
//! the shipped source and inspects the string literals directly.
//!
//! **The scanner is proved to bite.** `the_guard_catches_a_planted_ref` runs
//! it over a synthetic line and asserts it fires; a guard that has never been
//! seen to reject anything is a green light with no bulb.

const REF_MARKERS: [&str; 2] = ["WO-", "ADR-"];

/// Extracts the double-quoted string-literal contents from one line of Rust
/// source, ignoring `//` line comments (a comment is prose, not something a
/// caller ever receives). Handles escaped quotes; the guarded error/response
/// strings use no raw strings, so a plain scan is sufficient.
fn string_literals(line: &str) -> Vec<String> {
    let mut lits = Vec::new();
    let mut cur = String::new();
    let mut in_str = false;
    let mut escaped = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if in_str {
            if escaped {
                cur.push(c);
                escaped = false;
            } else if c == '\\' {
                cur.push(c);
                escaped = true;
            } else if c == '"' {
                lits.push(std::mem::take(&mut cur));
                in_str = false;
            } else {
                cur.push(c);
            }
        } else if c == '"' {
            in_str = true;
        } else if c == '/' && chars.peek() == Some(&'/') {
            break; // start of a line comment — nothing after it reaches a caller
        }
    }
    lits
}

/// Every `file:line` whose string literals carry an internal reference.
fn scan(src: &str) -> Vec<String> {
    let mut hits = Vec::new();
    for (i, line) in src.lines().enumerate() {
        for lit in string_literals(line) {
            if REF_MARKERS.iter().any(|m| lit.contains(m)) {
                hits.push(format!("{}: {}", i + 1, line.trim()));
            }
        }
    }
    hits
}

/// The whole plugin-route module: no string it can return may cite an internal
/// decision record. Covers the formerly-leaking `E_BAD_FILTER` message and any
/// response/error text added later.
#[test]
fn route_response_strings_carry_no_internal_refs() {
    let routes = include_str!("../src/routes.rs");
    let leaks = scan(routes);
    assert!(
        leaks.is_empty(),
        "internal WO-/ADR- reference in a routes.rs response/error string \
         (reword to product language):\n{}",
        leaks.join("\n")
    );
}

/// The specific bug this guard was written for: a malformed `filter` from a
/// caller (possibly unauthenticated) once drew an error naming an internal
/// decision record. The reworded text must describe the rule in product terms.
#[test]
fn the_formerly_leaking_filter_error_is_product_language() {
    let routes = include_str!("../src/routes.rs");
    let line = routes
        .lines()
        .find(|l| l.contains("E_BAD_FILTER"))
        .expect("the E_BAD_FILTER message must still exist");
    for marker in REF_MARKERS {
        assert!(
            !line.contains(marker),
            "E_BAD_FILTER response text must not contain {marker:?}: {line}"
        );
    }
}

/// A guard that cannot fail proves nothing: feed the scanner a line that leaks
/// and confirm it flags exactly that line.
#[test]
fn the_guard_catches_a_planted_ref() {
    let planted =
        r#"        return Err("FRUST:E_BAD_FILTER: filter must be a structured ADR-006 filter".into());"#;
    let hits = scan(planted);
    assert_eq!(
        hits.len(),
        1,
        "the scanner must flag a planted internal reference, else it guards nothing"
    );

    // And it must ignore a reference that lives only in a comment (prose is not
    // returned to a caller), or the guard would fight the doc-comment sweep.
    let comment_only = "        // ADR-006: filters are a typed tree, never text";
    assert!(
        scan(comment_only).is_empty(),
        "a reference confined to a comment is not caller-facing and must not trip the guard"
    );
}
