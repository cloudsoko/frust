// Tier-2 user script under test (~20 lines): reads fields, mutates its own
// doc, uses the brokered log() capability, and exercises the typed reject
// path. This is the script a Frappe user would store against a DocType.
//
// DECIMAL DISCIPLINE (WO-009 finding): money crosses the boundary as a
// STRING. Convert explicitly with Number() before arithmetic — `doc.total +
// tax` on the raw field is string concatenation and yields NaN — and write
// it back as a string so the shell re-imposes the decimal kind.
var total = Number(doc.total || 0);
if (total < 0) {
    throw "total must not be negative (got " + total + ")";
}
if (doc.id === "") {
    throw "document has no id";
}
if (doc.status === "Draft" && total > 10000) {
    log("large draft flagged: " + doc.id);
    doc.status = "Needs Approval";
}
// derived-field arithmetic with explicit rounding (money-ish)
var tax = Math.round(total * 0.15 * 100) / 100;
var grand = Math.round((total + tax) * 100) / 100;
if (grand > 1000000) {
    throw "grand total exceeds mandate limit";
}
if (doc.total !== undefined && doc.total !== null) {
    // give back the TYPE we were given: decimals arrive as strings and must
    // leave as strings; floats arrive as numbers and must leave as numbers
    doc.total = (typeof doc.total === "string") ? grand.toFixed(2) : grand;
}
if (doc.status !== "Draft" && doc.status !== "Needs Approval" && doc.status !== "Paid") {
    doc.status = "Draft";
}
doc;
