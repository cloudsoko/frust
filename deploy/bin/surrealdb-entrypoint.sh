#!/bin/sh
set -eu

read_secret() {
    name=$1
    file=$2
    if [ ! -r "$file" ]; then
        echo "configuration refused: secret $name is not readable at $file" >&2
        exit 78
    fi
    value=$(cat "$file")
    if [ -z "$value" ]; then
        echo "configuration refused: secret $name is empty" >&2
        exit 78
    fi
    printf '%s' "$value"
}

user="$(read_secret SURREAL_USER "${SURREAL_USER_FILE:-/run/secrets/surreal_root_username}")"
pass="$(read_secret SURREAL_PASS "${SURREAL_PASS_FILE:-/run/secrets/surreal_root_password}")"

if [ "$pass" = "root" ]; then
    echo "configuration refused: the development database password is not allowed in this image" >&2
    exit 78
fi

exec /usr/local/bin/surreal start surrealkv:/var/lib/surrealdb/data/frust \
    --bind 0.0.0.0:8000 \
    --log "${SURREAL_LOG:-info}" \
    --user "$user" \
    --pass "$pass" \
    --deny-net
