#!/bin/sh
set -eu
umask 077

here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
mkdir -p "$here/secrets"

if [ ! -f "$here/secrets/surreal_root_username.txt" ]; then
    printf 'root\n' > "$here/secrets/surreal_root_username.txt"
fi

if [ ! -f "$here/secrets/surreal_root_password.txt" ]; then
    if command -v openssl >/dev/null 2>&1; then
        openssl rand -hex 32 > "$here/secrets/surreal_root_password.txt"
    else
        od -An -N32 -tx1 /dev/urandom | tr -d ' \n' > "$here/secrets/surreal_root_password.txt"
        printf '\n' >> "$here/secrets/surreal_root_password.txt"
    fi
fi

printf 'Deployment secrets are present in %s\n' "$here/secrets"
