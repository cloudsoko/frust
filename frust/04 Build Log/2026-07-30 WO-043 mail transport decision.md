---
tags: [frust, build-log, batteries, email, decision, architecture, milestone-4]
created: 2026-07-30
status: RULED B (PM, 2026-07-30) — lettre directly, blocking, on a std::thread worker. ADR-004 decisive: option A pulls `topcoat-core`+`topcoat-view` into the kernel (= the kernel importing the UI framework, the headless-contract breach WO-042 just re-verified doesn't exist) + synthesises a fake `Cx`. B is also lazy-correct independently (+10 vs +45 crates, no async island, ADR-010 Tier-2 posture). Two corrections ratified: (1) "tokio-free" = request path not dep graph (kernel already pulls tokio via wasmtime-wasi); (2) the self-caught template-rejection (Cow<'static,str> accepts owned String). Finding banked: topcoat-mail is DESK-tier (needs a request Cx), lettre is the kernel-tier primitive — a clean kernel/Desk line.
work-order: "[[WO-043 Email Batteries]]"
---

# WO-043 — Which mail component the kernel wires

Escalated before writing code, per the WO's own clause ("report both options'
footprint … for me to rule on. Don't quietly let tokio into the core") and the
WO-008 identity-decision precedent.

**The escalation is not the one the WO anticipated.** It expected a *tokio*
question. Tokio turns out to be containable — and a smaller problem than the
one actually in the way.

## Three facts, measured not assumed

**1. The kernel is not tokio-free in the dependency sense — it never was.**

```
tokio v1.53.1 └── wasmtime-wasi v37.0.3 └── frust-kernel
```

So "tokio-free" (WO-024/025) precisely means **the request path is std-threads
and blocking `ureq`**, not that the crate is absent. That matters, because it
means "topcoat-mail pulls tokio" is *not*, by itself, a disqualifier.

**2. `topcoat-mail`'s `Transport::send` requires a `Cx` — a Topcoat *web
request context*.**

```rust
fn send<'a>(&'a self, cx: &'a Cx, mail: Mail) -> TransportFuture<'a>;
```

The `Cx` is not decoration: it renders the mail's HTML body, because
`Mail::html` is a `topcoat_view::View`. The crate is built to send mail **from
inside a request handler** — which is precisely what the kernel does not have.
Using it means the kernel gains `topcoat-core` + `topcoat-view` and synthesises
a fake request context to satisfy a signature. The kernel today has **zero**
Topcoat dependencies, and ADR-004 keeps it that way on purpose.

**3. `topcoat-mail`'s `smtp` feature *is* the async transport.**

```toml
smtp = ["lettre/smtp-transport", "lettre/tokio1", "lettre/tokio1-rustls-tls", "lettre/pool"]
```

There is no blocking SMTP path through `topcoat-mail`. Meanwhile in lettre
itself, the blocking transport is **ungated**:

```rust
#[cfg(any(feature = "tokio1", feature = "async-std1"))]
mod async_transport;      // async — feature-gated
mod transport;            // BLOCKING SmtpTransport — always present
```

and `smtp-transport` pulls no tokio of its own. **Verified by compiling it**,
not by reading the manifest: a blocking `SmtpTransport` constructs in an
isolated probe crate with `default-features = false`.

### One thing I expected to be a blocker and checked, and it is not

I nearly reported that a *metadata* template could never be a `Mail` body,
because `Mail::html` is a compile-time `View`. That is wrong.
`PartsWriter::push_str_unescaped` takes `impl Into<Cow<'static, str>>`, which
accepts an **owned `String`** — so a runtime template can become a `View` via
the same `Raw` wrapper WO-037 built (its `&'static str` was a *trust* choice,
not a limitation). Recording it because it would have been a confident, wrong
reason to reject an option.

## The trade, in numbers

Marginal crates added to the kernel's current 231:

| | **A — `topcoat-mail` + `smtp`** | **B — `lettre` blocking** |
|---|---|---|
| new crates | **+45** | **+10** |
| adds tokio to the graph | yes (`tokio`, `tokio-rustls`) | **no** |
| adds the web framework | **yes** (`topcoat-core`, `topcoat-view`) | no |
| needs a synthesised `Cx` | **yes** | no |
| SMTP call shape | `async`, needs a runtime on the worker | **blocking** — plain `std::thread` |
| body type | `View` (runtime string via `Raw`) | `String` natively |
| in the WO's sanctioned dep set | yes | **yes** (the WO names "topcoat-mail/lettre") |

Both are containable. Neither breaches the request path. The difference is
**4.5× the dependency surface and a web framework inside the kernel**, bought in
exchange for a body type the kernel does not want and an async call it must then
wrap a runtime around.

## Recommendation — B, lettre directly, blocking, on a std::thread worker

- It is the **ADR-010 Tier-2 posture exactly**: lifecycle event → enqueue →
  background std-thread drains it. No second runtime, no async island, nothing
  new to reason about.
- It keeps **ADR-004's lean kernel** — the boundary WO-042 just re-verified
  when it confirmed `frust_ui` appears nowhere in the vendored tree.
- It is inside the WO's stated dependency boundary, and it is the option the WO
  itself named as the alternative ("lettre's blocking transport").

**`topcoat-mail` is not wrong — it is built for a different tier.** It is the
right component the day the *Desk* wants to send view-rendered mail from a
request handler, where a `Cx` exists for free. Recommending against it here is
a statement about which side of the kernel/Desk line the mail worker sits on,
not about the crate.

## What "wire it, don't build it" still means under B

The WO's intent — do not write a mail library — holds either way. Under B the
kernel uses lettre's `Message` builder and blocking `SmtpTransport`; what
WO-043 writes is the **Frust** part, which is the actual work and is identical
under both options:

1. the Notification DocType (metadata, write-closed, no recompile),
2. rule evaluation on the lifecycle event,
3. the enqueue + draining worker,
4. field interpolation into the template,
5. file-vs-smtp selection by config,
6. bounded retry / dead-letter with typed `/metrics`.

## Status

**RULED B by the PM (2026-07-30).** lettre directly, blocking, on a std::thread
worker. Rationale: ADR-004 is decisive (option A puts the web framework inside a
kernel that ADR-004 keeps Topcoat-free — a load-bearing boundary, not crossed
for convenience) AND B is the lazy-correct answer on its own (+10 vs +45 crates,
no async island, the ADR-010 Tier-2 posture the WO already specified). Proceed to
build the Frust part (Notification DocType, rule eval, enqueue+worker,
interpolation, file/smtp config-selection, bounded retry + typed `/metrics`) over
lettre's `Message` builder + blocking transport. Confirm lettre's own
file/stub transport covers the CI-capture criterion (criterion 3) — no
`topcoat-mail` FileTransport needed. No kernel code written pre-ruling; probe was
in `/tmp` and is gone; tree untouched.

## Related
[[WO-043 Email Batteries]] · [[ADR-004 Topcoat for Desk v0]] (the lean-kernel
boundary this turns on) · [[ADR-010 Rollup Ladder]] (the worker posture) ·
[[2026-07-25 WO-024 load and footprint benchmark]] · [[Topcoat]] ·
[[2026-07-29 WO-042 frust ui re-skin]]
