// A thin CLIENT of the documented Frust REST surface (docs/rest-api.md).
// No SurrealDB, no kernel internals — just HTTP + JSON, exactly what
// byo-quickstart.md describes. Node 20+ has global fetch, so zero deps here.

export class FrustRest {
  constructor(base) {
    this.base = base.replace(/\/+$/, "");
    this.token = null; // set by login(); forwarded as Bearer on every call
  }

  async #call(method, path, { body, auth = true } = {}) {
    const headers = { "content-type": "application/json" };
    if (auth && this.token) headers.authorization = `Bearer ${this.token}`;
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
    return { ok: res.ok, status: res.status, json };
  }

  // /login is the auth-forwarding root: the kernel resolves+authenticates the
  // tenant and returns "<TenantId>.<random>". We hold it and send it back
  // verbatim. The prefix is the kernel's, not ours.
  async login(user, pass, tenant) {
    const body = { user, pass };
    if (tenant) body.tenant = tenant;
    const r = await this.#call("POST", "/login", { body, auth: false });
    if (!r.ok) {
      throw new Error(`login failed (HTTP ${r.status}): ${JSON.stringify(r.json)}`);
    }
    this.token = r.json.token;
    return r.json; // { token, user, role, tenant }
  }

  ready() { return this.#call("GET", "/ready", { auth: false }); }
  meta() { return this.#call("GET", "/meta"); }
  metaOne(dt) { return this.#call("GET", `/meta/${encodeURIComponent(dt)}`); }
  read(dt, body) { return this.#call("POST", `/read/${encodeURIComponent(dt)}`, { body: body ?? {} }); }
  write(dt, body) { return this.#call("POST", `/write/${encodeURIComponent(dt)}`, { body }); }
  workflow(dt, key) { return this.#call("GET", `/workflow/${encodeURIComponent(dt)}/${encodeURIComponent(key)}`); }
  transition(dt, key, action) {
    return this.#call("POST", `/transition/${encodeURIComponent(dt)}/${encodeURIComponent(key)}`, { body: { action } });
  }
}

// The kernel wants the bare key on write.record and /transition/{dt}/{key};
// reads hand back the full "doctype:key" id. Strip the table prefix if present
// so a caller may pass either form.
export function bareKey(id) {
  if (typeof id !== "string") return id;
  const i = id.indexOf(":");
  return i === -1 ? id : id.slice(i + 1);
}
