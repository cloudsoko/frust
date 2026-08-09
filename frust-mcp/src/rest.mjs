import { randomUUID } from "node:crypto";

// Thin client for the documented Frust REST surface. Authentication is always
// a kernel session token; the adapter never derives roles or permissions.
export class FrustRest {
  constructor(base, token = null) {
    this.base = base.replace(/\/+$/, "");
    this.token = token;
    this.lastTraceId = null;
  }

  async call(method, path, { body, auth = true, trace = true } = {}) {
    const headers = { "content-type": "application/json" };
    if (auth && this.token) headers.authorization = `Bearer ${this.token}`;
    if (trace) {
      this.lastTraceId = `mcp-${randomUUID()}`;
      headers["x-trace-id"] = this.lastTraceId;
    }
    const res = await fetch(this.base + path, {
      method,
      headers,
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    const text = await res.text();
    let json;
    try {
      json = text ? JSON.parse(text) : {};
    } catch {
      json = { raw: text };
    }
    return { ok: res.ok, status: res.status, json, traceId: this.lastTraceId };
  }

  async login(user, pass, tenant) {
    const body = { user, pass };
    if (tenant) body.tenant = tenant;
    const r = await this.call("POST", "/login", { body, auth: false });
    if (!r.ok) throw new Error(`login failed (HTTP ${r.status}): ${JSON.stringify(r.json)}`);
    this.token = r.json.token;
    return r.json;
  }

  ready() { return this.call("GET", "/ready", { auth: false, trace: false }); }
  meta() { return this.call("GET", "/meta"); }
  metaOne(dt) { return this.call("GET", `/meta/${encodeURIComponent(dt)}`); }
  read(dt, body) { return this.call("POST", `/read/${encodeURIComponent(dt)}`, { body: body ?? {} }); }
  write(dt, body) { return this.call("POST", `/write/${encodeURIComponent(dt)}`, { body }); }
  workflow(dt, key) { return this.call("GET", `/workflow/${encodeURIComponent(dt)}/${encodeURIComponent(key)}`); }
  transition(dt, key, action) {
    return this.call("POST", `/transition/${encodeURIComponent(dt)}/${encodeURIComponent(key)}`, { body: { action } });
  }
  subscribe(dt) { return this.call("POST", `/subscribe/${encodeURIComponent(dt)}`, { body: {} }); }
  events(sub) { return this.call("GET", `/events/${encodeURIComponent(sub)}`); }
  unsubscribe(sub) { return this.call("POST", `/unsubscribe/${encodeURIComponent(sub)}`, { body: {} }); }
}

export function bareKey(id) {
  if (typeof id !== "string") return id;
  const i = id.indexOf(":");
  return i === -1 ? id : id.slice(i + 1);
}
