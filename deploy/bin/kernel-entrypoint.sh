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

export FRUST_DB_ROOT_USER="$(read_secret FRUST_DB_ROOT_USER "${FRUST_DB_ROOT_USER_FILE:-/run/secrets/surreal_root_username}")"
export FRUST_DB_ROOT_PASS="$(read_secret FRUST_DB_ROOT_PASS "${FRUST_DB_ROOT_PASS_FILE:-/run/secrets/surreal_root_password}")"

case "${FRUST_DB_ENDPOINT:-}" in
    http://*|https://*) ;;
    *) echo "configuration refused: FRUST_DB_ENDPOINT must be an http(s) URL" >&2; exit 78 ;;
esac

case "${FRUST_DB_ACCESS:-account}" in
    *[!A-Za-z0-9_]*) echo "configuration refused: FRUST_DB_ACCESS must be a plain identifier" >&2; exit 78 ;;
esac

if [ "${FRUST_DB_ROOT_PASS}" = "root" ]; then
    echo "configuration refused: the development database password is not allowed in this image" >&2
    exit 78
fi

umask 077
mkdir -p "${FRUST_MAIL_DIR:-/var/lib/frust/mail-outbox}"
exec /usr/local/bin/frust "$@"
