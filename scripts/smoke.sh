#!/bin/sh
set -eu
binary=${1:-target/debug/qmodemd}
smoke_dir=$(mktemp -d /tmp/qmodem-smoke.XXXXXX)
smoke_pid=
cleanup() {
    if [ -n "$smoke_pid" ]; then kill "$smoke_pid" 2>/dev/null || true; wait "$smoke_pid" 2>/dev/null || true; fi
    case "$smoke_dir" in /tmp/qmodem-smoke.*) rm -rf -- "$smoke_dir" ;; esac
}
trap cleanup EXIT INT TERM
cat > "$smoke_dir/config.toml" <<CONFIG
version = 1
[server]
listen = "127.0.0.1"
port = 18088
[storage]
sqlite = "$smoke_dir/history.sqlite3"
CONFIG
"$binary" --config "$smoke_dir/config.toml" check
"$binary" --config "$smoke_dir/config.toml" set-service --listen 127.0.0.1 --port 18089
"$binary" --config "$smoke_dir/config.toml" service-info > "$smoke_dir/info.json"
node -e 'const d=JSON.parse(require("fs").readFileSync(process.argv[1])); if(d.port!==18089) process.exit(1)' "$smoke_dir/info.json"
"$binary" --config "$smoke_dir/config.toml" serve > "$smoke_dir/server.log" 2>&1 &
smoke_pid=$!
count=0
until curl -fsS http://127.0.0.1:18089/api/health > "$smoke_dir/health.json" 2>/dev/null; do
    kill -0 "$smoke_pid" || { cat "$smoke_dir/server.log"; exit 1; }
    count=$((count+1))
    [ "$count" -lt 50 ] || { cat "$smoke_dir/server.log"; exit 1; }
    sleep 0.1
done
node -e 'const d=JSON.parse(require("fs").readFileSync(process.argv[1])); if(d.service!=="qmodemd"||d.modem_api_ready!==false) process.exit(1)' "$smoke_dir/health.json"
test -s "$smoke_dir/history.sqlite3"
kill -TERM "$smoke_pid"
wait "$smoke_pid"
smoke_pid=
echo 'HTTP, TOML update, SQLite initialization and SIGTERM smoke checks passed'
