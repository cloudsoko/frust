function optionList(field) {
  return Array.isArray(field.options) ? field.options.filter((v) => typeof v === "string" && v.length) : [];
}

export function schemaForField(field, byName, seen = new Set()) {
  const p = {};
  let description = `${field.fieldname} (${field.fieldtype})`;
  switch (field.fieldtype) {
    case "Currency":
      p.type = "string";
      p.pattern = "^-?(?:0|[1-9]\\d*)(?:\\.\\d+)?$";
      description = `${field.fieldname} (Currency): exact decimal STRING; never send a JSON number`;
      break;
    case "Int": p.type = "integer"; break;
    case "Float": p.type = "number"; break;
    case "Check": p.type = "boolean"; break;
    case "Select": {
      p.type = "string";
      const options = optionList(field);
      if (options.length) p.enum = options;
      break;
    }
    case "Link": {
      p.type = "string";
      const targets = optionList(field);
      if (targets.length) {
        p["x-frust-link-doctypes"] = targets;
        if (targets.length === 1) p["x-frust-link-doctype"] = targets[0];
        description += `; target DocType${targets.length === 1 ? "" : "s"}: ${targets.join(", ")}`;
      }
      break;
    }
    case "Table": {
      p.type = "array";
      const childName = optionList(field)[0];
      const child = childName ? byName.get(childName) : undefined;
      if (child && !seen.has(childName)) {
        p.items = objectSchema(child, byName, new Set([...seen, childName]), false);
        p["x-frust-child-doctype"] = childName;
        description += `; embedded child rows of ${childName}`;
      } else {
        p.items = { type: "object" };
        description += childName ? `; child DocType ${childName}` : "; embedded child rows";
      }
      break;
    }
    default:
      p.type = "string";
  }
  p.description = description;
  return p;
}

export function objectSchema(doctype, byName, seen = new Set([doctype.name]), includeRequired = true) {
  const properties = {};
  const required = [];
  for (const field of doctype.fields ?? []) {
    properties[field.fieldname] = schemaForField(field, byName, seen);
    if (field.required) required.push(field.fieldname);
  }
  const schema = { type: "object", properties, additionalProperties: false };
  if (includeRequired && required.length) schema.required = required;
  return schema;
}

function encodeValue(field, value, byName, path) {
  if (value === null) return null;
  if (field.fieldtype === "Currency") {
    if (typeof value !== "string") throw new Error(`${path} must be a decimal string, never a JSON number`);
    if (!/^-?(?:0|[1-9]\d*)(?:\.\d+)?$/.test(value)) throw new Error(`${path} is not a decimal string`);
    return { kind: "decimal", v: value };
  }
  if (field.fieldtype === "Link") {
    if (typeof value !== "string") throw new Error(`${path} must be a record id string`);
    return { kind: "record", v: value };
  }
  if (field.fieldtype === "Table") {
    if (!Array.isArray(value)) throw new Error(`${path} must be an array of child rows`);
    const childName = optionList(field)[0];
    const child = childName ? byName.get(childName) : undefined;
    if (!child) return value;
    return value.map((row, index) => encodeDoc(childName, row, byName, `${path}[${index}]`));
  }
  return value;
}

export function encodeDoc(doctypeName, doc, byName, prefix = doctypeName) {
  if (!doc || typeof doc !== "object" || Array.isArray(doc)) throw new Error(`${prefix} must be an object`);
  const doctype = byName.get(doctypeName);
  if (!doctype) throw new Error(`unknown DocType '${doctypeName}'`);
  const fields = new Map((doctype.fields ?? []).map((field) => [field.fieldname, field]));
  const out = {};
  for (const [name, value] of Object.entries(doc)) {
    if (value === undefined) continue;
    const field = fields.get(name);
    out[name] = field ? encodeValue(field, value, byName, `${prefix}.${name}`) : value;
  }
  return out;
}

function assertValue(field, value, byName, path) {
  if (value == null) return;
  if (field.fieldtype === "Currency" && typeof value !== "string") {
    throw new Error(`kernel violated Currency wire contract at ${path}: received ${typeof value}`);
  }
  if (field.fieldtype === "Table" && Array.isArray(value)) {
    const childName = optionList(field)[0];
    if (childName && byName.has(childName)) {
      for (let i = 0; i < value.length; i++) assertWireDoc(childName, value[i], byName, `${path}[${i}]`);
    }
  }
}

export function assertWireDoc(doctypeName, doc, byName, prefix = doctypeName) {
  if (!doc || typeof doc !== "object") return doc;
  const doctype = byName.get(doctypeName);
  if (!doctype) return doc;
  for (const field of doctype.fields ?? []) assertValue(field, doc[field.fieldname], byName, `${prefix}.${field.fieldname}`);
  return doc;
}

export function assertWireRows(doctypeName, rows, byName) {
  for (let i = 0; i < (rows ?? []).length; i++) assertWireDoc(doctypeName, rows[i], byName, `${doctypeName}[${i}]`);
  return rows;
}
