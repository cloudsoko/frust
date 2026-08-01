#!/usr/bin/env bash
# WO-059 fixture, built through the REST door (BYO-client dogfood).
# Identities are pre-seeded (seed.surql); the doctype + workflow + rows come
# from here so the probe exercises the real kernel path.
set -euo pipefail
K=http://127.0.0.1:8795

login() { # user pass -> token
  curl -s -X POST "$K/login" -H 'Content-Type: application/json' \
    -d "{\"user\":\"$1\",\"pass\":\"$2\"}" \
    | python -c 'import sys,json;print(json.load(sys.stdin)["token"])'
}

MGR=$(login manager pw-manager)
C1=$(login clerk1 pw-clerk1)
C2=$(login clerk2 pw-clerk2)
echo "manager token prefix: ${MGR%%.*}   clerk1: ${C1%%.*}   clerk2: ${C2%%.*}"

echo "=== install expenses app (manager) ==="
curl -s -X POST "$K/app/install" -H "Authorization: Bearer $MGR" \
  -H 'Content-Type: application/json' -d @- <<'JSON' | python -m json.tool
{
  "manifest_version": 1,
  "name": "expenses",
  "version": "1.0.0",
  "doctypes": [{
    "name": "expense_claim",
    "submittable": true,
    "fields": [
      { "fieldname": "purpose", "label": "Purpose", "fieldtype": "Data", "required": true },
      { "fieldname": "amount", "label": "Claim Amount", "fieldtype": "Currency", "required": true },
      { "fieldname": "workflow_state", "fieldtype": "Data" }
    ]
  }],
  "workflows": [{
    "name": "expense_approval",
    "doctype": "expense_claim",
    "states": [
      { "name": "Draft", "docstatus": 0 },
      { "name": "Submitted for Approval", "docstatus": 0 },
      { "name": "Approved", "docstatus": 1 },
      { "name": "Rejected", "docstatus": 0 }
    ],
    "transitions": [
      { "from": "Draft", "to": "Submitted for Approval", "role": "clerk", "action": "Submit" },
      { "from": "Submitted for Approval", "to": "Approved", "role": "manager", "action": "Approve" },
      { "from": "Submitted for Approval", "to": "Rejected", "role": "manager", "action": "Reject" },
      { "from": "Rejected", "to": "Draft", "role": "clerk", "action": "Reopen" }
    ],
    "state_rules": [
      { "state": "Submitted for Approval", "field": "amount", "read_only": true },
      { "state": "Approved", "field": "amount", "read_only": true }
    ]
  }]
}
JSON

create() { # token purpose amount
  # money goes in the TYPED decimal form — a Currency field REFUSES a bare
  # decimal string on write ("Expected decimal but found '42.00'"); this is the
  # WO-059 money-safety finding. Integers or {kind:decimal,v} are accepted.
  curl -s -X POST "$K/write/expense_claim" -H "Authorization: Bearer $1" \
    -H 'Content-Type: application/json' \
    -d "{\"doc\":{\"purpose\":\"$2\",\"amount\":{\"kind\":\"decimal\",\"v\":\"$3\"},\"workflow_state\":\"Draft\"}}" \
    | python -c 'import sys,json;d=json.load(sys.stdin);print(d.get("action"),d.get("record"),d.get("error",""))'
}

echo "=== clerk1 creates 2 claims ==="
create "$C1" "Taxi to client site"   "42.00"
create "$C1" "Team lunch"            "118.50"
echo "=== clerk2 creates 2 claims ==="
create "$C2" "Conference ticket"     "560.00"
create "$C2" "Hotel night"           "205.00"
echo "=== manager creates 1 claim ==="
create "$MGR" "Manager offsite"      "999.00"

echo "=== provenance sanity: who sees what (owner field) ==="
seerows() { curl -s -X POST "$K/read/expense_claim" -H "Authorization: Bearer $1" \
  -H 'Content-Type: application/json' -d '{"fields":["purpose","amount","owner","workflow_state"]}' ; }
echo "clerk1 sees:";  seerows "$C1" | python -m json.tool
echo "clerk2 sees:";  seerows "$C2" | python -m json.tool
echo "manager sees (count):"; seerows "$MGR" | python -c 'import sys,json;print(len(json.load(sys.stdin)["rows"]),"rows")'
