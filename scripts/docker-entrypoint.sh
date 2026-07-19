#!/bin/sh
set -eu

runtime_uid="${PUID:-10001}"
runtime_gid="${PGID:-10001}"
database_path="${UNUNKNOWN_DB:-/data/cache/ununknown.sqlite}"
cache_dir="$(dirname "$database_path")"
output_dir="${UNUNKNOWN_OUTPUT_DIR:-/data/output}"
input_dir="${UNUNKNOWN_INPUT_DIR:-/data/input}"
input_mode="${UNUNKNOWN_INPUT_MODE:-auto}"

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

detect_input_mode() {
    awk -v target="$input_dir" '
        $5 == target {
            count = split($6, options, ",")
            for (option_index = 1; option_index <= count; option_index++) {
                if (options[option_index] == "rw") {
                    print "rw"
                    found = 1
                    exit
                }
            }
            print "ro"
            found = 1
            exit
        }
        END {
            if (!found) print "ro"
        }
    ' /proc/self/mountinfo
}

case "$input_mode" in
    auto)
        input_mode="$(detect_input_mode)"
        ;;
    ro|rw)
        ;;
    *)
        echo "Error: UNUNKNOWN_INPUT_MODE must be 'ro', 'rw', or 'auto'" >&2
        exit 64
        ;;
esac

prepare_writable_input_directories() {
    wanted_owner="${runtime_uid}:${runtime_gid}"

    mkdir -p "$input_dir"
    echo "Preparing writable input directories for $wanted_owner"
    find "$input_dir" -xdev -type d \( ! -user "$runtime_uid" -o ! -group "$runtime_gid" \) \
        -exec chown "$wanted_owner" {} +
    find "$input_dir" -xdev -type d ! -perm -0700 -exec chmod u+rwx {} +
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
    if [ "$input_mode" = "rw" ] && [ ! -w "$input_dir" ]; then
        echo "Error: writable input directory $input_dir is not writable" >&2
        exit 77
    fi
    exec "$@"
fi

prepare_writable_tree "$cache_dir"
prepare_writable_tree "$output_dir"
if [ "$input_mode" = "rw" ]; then
    prepare_writable_input_directories
fi

exec su-exec "${runtime_uid}:${runtime_gid}" "$@"
