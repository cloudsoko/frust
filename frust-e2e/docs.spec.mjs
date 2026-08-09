/**
 * WO-054: the REST docs, executed.
 *
 * "An example you can't re-run is an anecdote" is this project's rule for
 * evidence, and docs get it too. Every request shape in `frust-kernel/docs/`
 * runs here against a live `frust serve`, and the documented route table is
 * cross-checked against the routes extracted from `rest.rs` — so a route added
 * without documentation fails, and a documented route that no longer exists
 * fails.
 *
 *   node docs.spec.mjs            (kernel on :8790, seeded dev store)
 */
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const HERE = dirname(fileURLToPath(import.meta.url));
const KERNEL = process.env.FRUST_KERNEL ?? 'http://127.0.0.1:8790';
const REPO = join(HERE, '..');

let pass = 0, fail = 0;
const failures = [];

function check(name, cond, detail = '') {
  if (cond) { pass++; console.log(`  ok   ${name}`); }
  else { fail++; failures.push(`${name}${detail ? ' — ' + detail : ''}`); console.log(`  FAIL ${name} ${detail}`); }
}

async function call(path, { token, body, method } = {}) {
  const res = await fetch(`${KERNEL}${path}`, {
    method: method ?? (body === undefined ? 'GET' : 'POST'),
    headers: {
      'Content-Type': 'application/json',
      ...(token ? { Authorization: `Bearer ${token}` } : {}),
    },
    ...(body === undefined ? {} : { body: JSON.stringify(body) }),
  });
  let json = null;
  try { json = await res.json(); } catch { /* /metrics is text */ }
  return { status: res.status, json, res };
}

// ── 1. the route table cannot drift from the code ──────────────────────────
//
// The strongest anti-rot device available: parse both sides and compare sets.
// A doc that merely *reads* correct is a doc that rots silently.

function routesFromSource() {
  const src = readFileSync(join(REPO, 'frust-kernel/kernel/src/rest.rs'), 'utf8');
  const out = new Set();
  for (const m of src.matchAll(/^\s{8,16}\[([^\]]*)\] =>/gm)) {
    // ["app", name, "disable"]  ->  /app/{}/disable
    const segs = m[1].split(',').map(s => s.trim()).map(s =>
      s.startsWith('"') ? s.slice(1, -1) : '{}');
    out.add('/' + segs.join('/'));
  }
  // handled before routing, so it is not a match arm
  out.add('/metrics');
  return out;
}

function routesFromDocs() {
  const md = readFileSync(join(REPO, 'frust-kernel/docs/rest-api.md'), 'utf8');
  const out = new Set();
  // headings like:  ### `POST /write/{doctype}` — session
  for (const m of md.matchAll(/^###\s+`([A-Z|]+\s+)?([^`]+)`/gm)) {
    for (const p of m[2].split('|')) out.add(normalise(p.trim()));
  }
  // table rows like: | `POST /doctype` | … |
  for (const m of md.matchAll(/^\|\s*`(?:[A-Z|]+\s+)?(\/[^`]+)`\s*\|/gm)) {
    out.add(normalise(m[1].trim()));
  }
  out.delete('');
  return out;
}

const normalise = p => p.replace(/^[A-Z|]+\s+/, '').replace(/\{[^}]*\}/g, '{}').replace(/\/$/, '');

function routeCoverage() {
  console.log('\n== route table vs rest.rs ==');
  const src = routesFromSource();
  const docs = routesFromDocs();
  // `/{}/{}` style catch-alls in source that the docs describe under a name
  const known = new Set([...src].filter(r => r !== '/{}'));
  const undocumented = [...known].filter(r => !docs.has(r));
  const phantom = [...docs].filter(r => !known.has(r));
  check(`every route in rest.rs is documented (${known.size} routes)`,
    undocumented.length === 0, `undocumented: ${undocumented.join(', ')}`);
  check('no documented route is missing from rest.rs',
    phantom.length === 0, `not in source: ${phantom.join(', ')}`);
}

// ── 2. every documented example executes ───────────────────────────────────

async function main() {
  routeCoverage();

  console.log('\n== no-auth tier ==');
  const health = await call('/health');
  check('GET /health -> 200 {ok:true}', health.status === 200 && health.json?.ok === true,
    JSON.stringify(health.json));

  const metrics = await fetch(`${KERNEL}/metrics`);
  const mtext = await metrics.text();
  check('GET /metrics -> 200 prometheus text',
    metrics.status === 200 && /^# TYPE /m.test(mtext),
    `status=${metrics.status}`);

  const ready = await call('/ready');
  check('GET /ready -> 200 {ready:true, tenants:[…]}',
    ready.status === 200 && ready.json?.ready === true &&
    Array.isArray(ready.json?.tenants) && ready.json.tenants.length > 0,
    JSON.stringify(ready.json));
  check('/ready carries the boot facts, not just a flag',
    Number.isInteger(ready.json?.tenants?.[0]?.meta_version) &&
    Number.isInteger(ready.json?.tenants?.[0]?.doctypes),
    JSON.stringify(ready.json?.tenants?.[0]));

  const noAuth = await call('/read/sales_invoice', { body: {} });
  check('unauthenticated -> 401 E_UNAUTHENTICATED',
    noAuth.status === 401 && noAuth.json?.error?.detail === 'E_UNAUTHENTICATED',
    JSON.stringify(noAuth.json));

  const badJson = await fetch(`${KERNEL}/read/sales_invoice`, {
    method: 'POST', headers: { 'Content-Type': 'application/json' }, body: '{not json',
  });
  const badJsonBody = await badJson.json();
  check('malformed body -> 400 bad json body',
    badJson.status === 400 && /bad json body/.test(badJsonBody?.error?.detail ?? ''),
    JSON.stringify(badJsonBody));

  // WO-054 rider (a): a body that is not UTF-8 names the actual fault
  const badUtf8 = await fetch(`${KERNEL}/read/sales_invoice`, {
    method: 'POST', headers: { 'Content-Type': 'application/json' },
    body: new Uint8Array([0x7b, 0x22, 0x61, 0x22, 0x3a, 0x22, 0xff, 0xfe, 0x22, 0x7d]),
  });
  const badUtf8Body = await badUtf8.json();
  check('non-UTF-8 body -> 400 naming UTF-8 (not a misleading field error)',
    badUtf8.status === 400 && /not valid UTF-8/.test(badUtf8Body?.error?.detail ?? ''),
    JSON.stringify(badUtf8Body));

  console.log('\n== login ==');
  const login = await call('/login', { body: { user: 'manager', pass: 'pw-manager' } });
  const token = login.json?.token;
  check('POST /login -> 200 {token,user,role,tenant}',
    login.status === 200 && !!token && login.json.user === 'manager' &&
    login.json.role === 'manager' && !!login.json.tenant,
    JSON.stringify(login.json));
  check('token is <TenantId>.<random> as documented',
    typeof token === 'string' && token.includes('.') &&
    token.split('.')[0] === login.json.tenant,
    token);

  // G1 is FIXED (WO-055), so the status is a promise now and asserted as one.
  const badCreds = await call('/login', { body: { user: 'manager', pass: 'wrong' } });
  check('bad credentials -> 401 FRUST:E_AUTH_REJECTED',
    badCreds.status === 401 && badCreds.json?.error?.detail === 'FRUST:E_AUTH_REJECTED' &&
    !badCreds.json?.token, JSON.stringify(badCreds.json));
  check('the refusal leaks no transport detail',
    !/http status|signin transport|404/.test(JSON.stringify(badCreds.json)),
    JSON.stringify(badCreds.json));

  const ghost = await call('/login', { body: { user: 'no_such_user', pass: 'x' } });
  check('an unknown user is indistinguishable from a wrong password',
    JSON.stringify(ghost.json) === JSON.stringify(badCreds.json),
    `${JSON.stringify(ghost.json)} vs ${JSON.stringify(badCreds.json)}`);

  const clerk = await call('/login', { body: { user: 'clerk1', pass: 'pw-clerk1' } });
  const clerkToken = clerk.json?.token;
  check('clerk logs in with role clerk', clerk.status === 200 && clerk.json?.role === 'clerk',
    JSON.stringify(clerk.json));

  console.log('\n== session tier ==');
  const meta = await call('/meta', { token });
  check('GET /meta -> 200 {doctypes:[…]}',
    meta.status === 200 && Array.isArray(meta.json?.doctypes) && meta.json.doctypes.length > 0,
    JSON.stringify(meta.json).slice(0, 120));

  const metaOne = await call('/meta/sales_invoice', { token });
  check('GET /meta/{doctype} -> 200 {doctype:{…}}',
    metaOne.status === 200 && !!metaOne.json?.doctype, JSON.stringify(metaOne.json).slice(0, 120));
  const customerMeta = metaOne.json?.doctype?.fields?.find(f => f.fieldname === 'customer');
  check('/meta/{doctype} exposes field label text by value',
    customerMeta?.label === 'Customer', JSON.stringify(customerMeta));
  check('/meta/{doctype} exposes the attached workflow states and transitions by value',
    metaOne.json?.doctype?.workflow?.name === 'invoice_approval' &&
    metaOne.json?.doctype?.workflow?.states?.some(s => s.name === 'Draft' && s.docstatus === 0) &&
    metaOne.json?.doctype?.workflow?.transitions?.some(t =>
      t.from === 'Draft' && t.to === 'Submitted for Approval' &&
      t.role === 'clerk' && t.action === 'Submit'),
    JSON.stringify(metaOne.json?.doctype?.workflow).slice(0, 240));

  const read = await call('/read/sales_invoice', {
    token,
    body: { fields: ['customer', 'total'], order: { path: 'total', dir: 'desc' }, limit: 3 },
  });
  check('POST /read/{doctype} -> 200 {rows:[…]}',
    read.status === 200 && Array.isArray(read.json?.rows), JSON.stringify(read.json).slice(0, 120));
  check('read honours limit', (read.json?.rows?.length ?? 99) <= 3, `${read.json?.rows?.length} rows`);

  const filtered = await call('/read/sales_invoice', {
    token, body: { filter: { path: 'customer', op: 'eq', value: 'Northwind Traders' }, limit: 5 },
  });
  check('structured filter -> 200 and only matching rows',
    filtered.status === 200 &&
    (filtered.json?.rows ?? []).every(r => r.customer === 'Northwind Traders'),
    JSON.stringify(filtered.json).slice(0, 160));

  const injected = await call('/read/sales_invoice', {
    token, body: { filter: { path: 'customer', op: 'eq', value: "x'; REMOVE TABLE sales_invoice; --" } },
  });
  check('query text in a filter VALUE is data, never an escape',
    injected.status === 200 && (injected.json?.rows ?? []).length === 0,
    `status=${injected.status}`);

  const unknown = await call('/read/no_such_doctype', { token, body: {} });
  check('unknown doctype -> 404 unknown-doctype',
    unknown.status === 404 && unknown.json?.error?.kind === 'unknown-doctype',
    JSON.stringify(unknown.json));

  console.log('\n== write, money, and hooks ==');
  const created = await call('/write/sales_invoice', {
    token: clerkToken,
    body: { doc: { customer: 'Docs Harness', total: '25.125',
                   lines: [{ item: 'Sprocket', qty: '1', rate: '25.125', amount: '25.125' }] } },
  });
  const rec = created.json?.created;
  check('POST /write/{doctype} create -> 200 {action,record,created}',
    created.status === 200 && !!rec?.id && created.json?.action === 'created' &&
    created.json?.record === rec?.id,
    JSON.stringify(created.json).slice(0, 220));
  check('money reads back as a STRING, not a float',
    typeof rec?.total === 'string', `total=${JSON.stringify(rec?.total)} (${typeof rec?.total})`);
  check('a bare decimal Currency write persists the exact string',
    rec?.total === '25.125', `total=${JSON.stringify(rec?.total)}`);
  check('a new document starts at docstatus 0', rec?.docstatus === 0, `docstatus=${rec?.docstatus}`);

  const key = rec?.id?.split(':')[1];
  const typedId = await call('/read/sales_invoice', {
    token, body: { filter: { path: 'id', op: 'eq', value: { kind: 'record', v: rec?.id } } },
  });
  // The bare key (no `doctype:` prefix) exercises the coercion that scopes an
  // unqualified id to the route's DocType; a qualified string would skip it.
  const plainId = await call('/read/sales_invoice', {
    token, body: { filter: { path: 'id', op: 'eq', value: key } },
  });
  check('a bare-key id filter returns the same row as the typed record filter',
    typedId.status === 200 && plainId.status === 200 &&
    typedId.json?.rows?.length === 1 &&
    JSON.stringify(plainId.json?.rows) === JSON.stringify(typedId.json?.rows),
    `${JSON.stringify(plainId.json)} vs ${JSON.stringify(typedId.json)}`);

  const updated = await call('/write/sales_invoice', {
    token: clerkToken, body: { record: key, doc: { customer: 'Docs Harness II' } },
  });
  check('update (record present) -> 200, partial fields preserved',
    updated.status === 200 && updated.json?.created?.customer === 'Docs Harness II' &&
    updated.json?.created?.total === rec?.total,
    JSON.stringify(updated.json).slice(0, 200));
  check('an update says action:"updated" (G3 — `created` said otherwise)',
    updated.json?.action === 'updated', JSON.stringify(updated.json).slice(0, 160));

  // G4: an unknown key is refused by name, not discarded
  const strayKey = await call('/write/sales_invoice', {
    token: clerkToken, body: { op: 'create', doc: { customer: 'nope' } },
  });
  check('an unknown write field -> 400 FRUST:E_UNKNOWN_FIELD naming it',
    strayKey.status === 400 && /E_UNKNOWN_FIELD/.test(strayKey.json?.error?.detail ?? '') &&
    /'op'/.test(strayKey.json?.error?.detail ?? ''),
    JSON.stringify(strayKey.json).slice(0, 220));

  const unbalanced = await call('/write/sales_invoice', {
    token: clerkToken, body: { doc: { customer: 'Docs Harness', total: 999.00, lines: [] } },
  });
  check('an app rule refusing a write -> 422 hook-rejected naming its app',
    unbalanced.status === 422 && unbalanced.json?.error?.kind === 'hook-rejected' &&
    /app '/.test(unbalanced.json?.error?.message ?? ''),
    JSON.stringify(unbalanced.json).slice(0, 200));

  // WO-057: a write the database refuses must not report success
  const refused = await call('/write/ar_outstanding', {
    token, body: { doc: { k: 'Docs Harness Probe', charged: '1', paid: '0' } },
  });
  check('a write-closed table refuses a create -> 403 E_WRITE_NO_ROWS',
    refused.status === 403 && /E_WRITE_NO_ROWS/.test(refused.json?.error?.detail ?? ''),
    JSON.stringify(refused.json).slice(0, 200));
  check('the refusal is not dressed as a success',
    refused.json?.action !== 'created' && !refused.json?.record,
    JSON.stringify(refused.json).slice(0, 160));

  console.log('\n== workflow ==');
  const actions = await call(`/workflow/sales_invoice/${key}`, { token: clerkToken });
  check('GET /workflow/{doctype}/{key} -> the caller\'s available actions',
    actions.status === 200, JSON.stringify(actions.json).slice(0, 160));

  const submitted = await call(`/transition/sales_invoice/${key}`, {
    token: clerkToken, body: { action: 'Submit' },
  });
  check('POST /transition -> 200, and Submit leaves docstatus 0',
    submitted.status === 200 && submitted.json?.docstatus === 0 &&
    submitted.json?.workflow_state === 'Submitted for Approval',
    JSON.stringify(submitted.json).slice(0, 200));

  const wrongRole = await call(`/transition/sales_invoice/${key}`, {
    token: clerkToken, body: { action: 'Approve' },
  });
  check('a role-denied transition -> 422 workflow-denied with a FRUST code',
    wrongRole.status === 422 && wrongRole.json?.error?.kind === 'workflow-denied' &&
    /^FRUST:E_WORKFLOW:/.test(wrongRole.json?.error?.code ?? ''),
    JSON.stringify(wrongRole.json));

  const approved = await call(`/transition/sales_invoice/${key}`, {
    token, body: { action: 'Approve' },
  });
  check('manager Approve -> 200 and docstatus crosses to 1',
    approved.status === 200 && approved.json?.docstatus === 1,
    JSON.stringify(approved.json).slice(0, 200));

  const badAction = await call(`/transition/sales_invoice/${key}`, {
    token, body: { action: 'Nope' },
  });
  check('unknown action -> 422 FRUST:E_WORKFLOW:UNKNOWN_ACTION',
    badAction.json?.error?.code === 'FRUST:E_WORKFLOW:UNKNOWN_ACTION',
    JSON.stringify(badAction.json));

  const noAction = await call(`/transition/sales_invoice/${key}`, { token, body: {} });
  check('transition without an action -> 400 invalid-value',
    noAction.status === 400 && /needs an 'action'/.test(noAction.json?.error?.detail ?? ''),
    JSON.stringify(noAction.json));

  console.log('\n== aggregate ==');
  const agg = await call('/aggregate/sales_invoice', {
    token, body: { group_by: ['customer'], metrics: [{ metric: 'sum', path: 'total' }] },
  });
  check('POST /aggregate -> 200 {rows:[…]}',
    agg.status === 200 && Array.isArray(agg.json?.rows), JSON.stringify(agg.json).slice(0, 160));

  console.log('\n== realtime ==');
  const sub = await call('/subscribe/sales_invoice', { token, method: 'POST' });
  check('POST /subscribe -> 200 {sub, budget}',
    sub.status === 200 && !!sub.json?.sub && typeof sub.json?.budget === 'number',
    JSON.stringify(sub.json));
  const events = await call(`/events/${sub.json?.sub}`, { token });
  check('GET /events/{sub} -> 200 {alive, events:[…]}',
    events.status === 200 && events.json?.alive === true && Array.isArray(events.json?.events),
    JSON.stringify(events.json).slice(0, 160));
  const unsub = await call(`/unsubscribe/${sub.json?.sub}`, { token, method: 'POST' });
  check('POST /unsubscribe -> 200 {ok:true}', unsub.status === 200 && unsub.json?.ok === true,
    JSON.stringify(unsub.json));

  console.log('\n== manager tier ==');
  const apps = await call('/app', { token });
  check('GET /app -> 200 {apps:[…]}', apps.status === 200 && Array.isArray(apps.json?.apps),
    JSON.stringify(apps.json).slice(0, 160));

  const audit = await call(`/audit/sales_invoice/${key}`, { token });
  check('GET /audit/{doctype}/{key} -> 200 {record,total,entries}',
    audit.status === 200 && Array.isArray(audit.json?.entries),
    JSON.stringify(audit.json).slice(0, 160));

  const outbox = await call('/mail/outbox', { token });
  check('GET /mail/outbox -> 200 {outbox:[…]}',
    outbox.status === 200 && Array.isArray(outbox.json?.outbox),
    JSON.stringify(outbox.json).slice(0, 120));

  const clerkOnManager = await call('/app', { token: clerkToken });
  check('manager route as clerk -> 403 manager role required',
    clerkOnManager.status === 403 && /manager role required/.test(clerkOnManager.json?.error?.detail ?? ''),
    JSON.stringify(clerkOnManager.json));

  const reclaimNoAck = await call('/doctype/sales_invoice/reclaim', {
    token, body: { column: 'crm_followup' },
  });
  check('reclaim without acknowledge -> refused, naming column and rows',
    reclaimNoAck.status >= 400 &&
    /crm_followup/.test(reclaimNoAck.json?.error?.detail ?? '') &&
    /row/.test(reclaimNoAck.json?.error?.detail ?? ''),
    JSON.stringify(reclaimNoAck.json).slice(0, 200));

  console.log('\n== unknown route ==');
  const nope = await call('/no/such/route', { token, body: {} });
  check('unknown path -> 400 no route', nope.status === 400 && /no route/.test(nope.json?.error?.detail ?? ''),
    JSON.stringify(nope.json));

  console.log('\n== logout ends the session ==');
  const lo = await call('/logout', { token: clerkToken, body: {} });
  check('POST /logout -> 200 {ok:true}', lo.status === 200 && lo.json?.ok === true,
    JSON.stringify(lo.json));
  const afterLogout = await call('/meta', { token: clerkToken });
  check('the token is dead immediately after logout', afterLogout.status === 401,
    `status=${afterLogout.status}`);

  console.log(`\n${'='.repeat(60)}\ndocs examples: ${pass} passed, ${fail} failed`);
  if (fail) { console.log('\nfailures:'); failures.forEach(f => console.log('  - ' + f)); process.exit(1); }
}

main().catch(e => { console.error(e); process.exit(1); });
