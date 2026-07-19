#!/bin/sh
set -eu

runtime_uid="${PUID:-10001}"
runtime_gid="${PGID:-10001}"
database_path="${UNUNKNOWN_DB:-/data/cache/ununknown.sqlite}"
cache_dir="$(dirname "$database_path")"
output_dir="${UNUNKNOWN_OUTPUT_DIR:-/data/output}"

validate_id() {
    name="$1"
    value="$2"

    case "$value" in
        ''|*[!0-9]*)
            echo "Error: $name must be a positive numeric ID, got '$value'" >&2
            exit 64
            ;;
    esac

    if [ "$value" = "0" ]; then
        echo "Error: $name must be greater than zero" >&2
        exit 64
    fi
}

validate_id PUID "$runtime_uid"
validate_id PGID "$runtime_gid"

prepare_writable_tree() {
    path="$1"
    wanted_owner="${runtime_uid}:${runtime_gid}"

    mkdir -p "$path"
    current_owner="$(stat -c '%u:%g' "$path")"
    if [ "$current_owner" != "$wanted_owner" ]; then
        echo "Setting ownership of $path to $wanted_owner"
        chown -R "$wanted_owner" "$path"
    fi
    chmod u+rwx "$path"
}

current_uid="$(id -u)"
current_gid="$(id -g)"

if [ "$current_uid" != "0" ]; then
    if [ "$current_uid" != "$runtime_uid" ] || [ "$current_gid" != "$runtime_gid" ]; then
        echo "Error: remove the Compose 'user:' override; use PUID and PGID instead" >&2
        exit 77
    fi
    if [ ! -w "$cache_dir" ] || [ ! -w "$output_dir" ]; then
        echo "Error: $cache_dir and $output_dir must be writable" >&2
        exit 77
    fi
    exec "$@"
fi

prepare_writable_tree "$cache_dir"
prepare_writable_tree "$output_dir"

exec su-exec "${runtime_uid}:${runtime_gid}" "$@"
