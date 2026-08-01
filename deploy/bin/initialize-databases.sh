#!/bin/sh
set -eu

read_secret() {
    file=$1
    if [ ! -r "$file" ]; then
        echo "database initialization refused: secret is not readable at $file" >&2
        exit 78
    fi
    value=$(cat "$file")
    if [ -z "$value" ]; then
        echo "database initialization refused: secret at $file is empty" >&2
        exit 78
    fi
    printf '%s' "$value"
}

plain_identifier() {
    case "$1" in
        ''|*[!A-Za-z0-9_]*) return 1 ;;
        *) return 0 ;;
    esac
}

if [ "${FRUST_TENANCY:-database-per-tenant}" != "database-per-tenant" ]; then
    echo "database initialization refused: this deployment foundation supports database-per-tenant only" >&2
    exit 78
fi

namespace=${FRUST_NS:-frust}
plain_identifier "$namespace" || {
    echo "database initialization refused: FRUST_NS must be a plain identifier" >&2
    exit 78
}

user="$(read_secret "${SURREAL_USER_FILE:-/run/secrets/surreal_root_username}")"
pass="$(read_secret "${SURREAL_PASS_FILE:-/run/secrets/surreal_root_password}")"
tenants=${FRUST_TENANTS:-site}

sql="DEFINE NAMESPACE IF NOT EXISTS $namespace; USE NS $namespace;"
old_ifs=$IFS
IFS=,
for tenant in $tenants; do
    tenant=$(printf '%s' "$tenant" | sed 's/^[[:space:]]*//;s/[[:space:]]*$//')
    plain_identifier "$tenant" || {
        echo "database initialization refused: every FRUST_TENANTS entry must be a plain identifier" >&2
        exit 78
    }
    sql="$sql DEFINE DATABASE IF NOT EXISTS $tenant;"
done
IFS=$old_ifs

printf '%s\n' "$sql" | /usr/local/bin/surreal sql \
    --endpoint "${SURREAL_ENDPOINT:-http://database:8000}" \
    --username "$user" \
    --password "$pass" \
    --hide-welcome
