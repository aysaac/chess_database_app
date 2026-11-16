#!/usr/bin/env bash
# Runs chess-server behind Caddy on this machine, configured as
# Caddyfile.example is (only the site address and the upstream differ, and
# Caddy's own local CA stands in for Let's Encrypt), and checks what the
# deployment depends on: HTTPS, the headers Caddy adds, Secure cookies, the
# Origin check, the client's address from X-Forwarded-For (login
# throttling), an upload and a compressed streamed export through the proxy.
#
#   server/deploy/check-behind-caddy.sh        # from the repository root; needs caddy and curl
#
# Everything is in a temporary folder (data, Caddy's certificates) and on
# 127.0.0.1 ports 18080 and 18443; both processes are stopped at the end.
# The app's own client (chess-net, as `examples/server_copy.rs`) makes and
# updates a copy through it too. A real certificate (Let's Encrypt for a
# public domain) can't be checked
# this way: see docs/server.md (In a container, behind Caddy).
set -euo pipefail

cd "$(dirname "$0")/../.."
command -v caddy >/dev/null || { echo "needs caddy (https://caddyserver.com)"; exit 2; }
work=$(mktemp -d)
server_pid=""
caddy_pid=""
cleanup() {
  [[ -n $caddy_pid ]] && kill "$caddy_pid" 2>/dev/null || true
  [[ -n $server_pid ]] && kill "$server_pid" 2>/dev/null || true
  wait 2>/dev/null || true
  rm -rf "$work"
}
trap cleanup EXIT

site="localhost:18443"
base="https://$site"
fails=0
check() { # check NAME CONDITION...
  local name=$1; shift
  if "$@"; then echo "ok   $name"; else echo "FAIL $name"; fails=$((fails + 1)); fi
}

cargo build -q -p chess-server
mkdir -p "$work/data"
CHESS_SERVER_DATA="$work/data" CHESS_SERVER_BIND=127.0.0.1:18080 \
  CHESS_SERVER_PUBLIC_URL="$base" CHESS_SERVER_TRUST_PROXY=1 CHESS_SERVER_MAX_UPLOAD_MB=256 \
  target/debug/chess-server serve >"$work/server.log" 2>&1 &
server_pid=$!

# The example's directives, for this address, with Caddy's local CA.
sed -e "s|^chess.example.com {|$site {\n\ttls internal|" -e 's|reverse_proxy chess-server:8080|reverse_proxy 127.0.0.1:18080|' \
  server/deploy/Caddyfile.example >"$work/Caddyfile"
XDG_DATA_HOME="$work/caddy-data" XDG_CONFIG_HOME="$work/caddy-config" \
  caddy run --config "$work/Caddyfile" --adapter caddyfile >"$work/caddy.log" 2>&1 &
caddy_pid=$!

ca="$work/caddy-data/caddy/pki/authorities/local/root.crt"
for _ in $(seq 100); do
  [[ -f $ca ]] && curl -s --cacert "$ca" "$base/api/health" >/dev/null 2>&1 && break
  sleep 0.2
done
c() { curl -s --cacert "$ca" "$@"; }

check "HTTPS answers with the server's health" grep -q '"ok"' <(c "$base/api/health")
headers=$(c -D - -o /dev/null "$base/api/health")
check "Caddy adds HSTS" grep -qi '^strict-transport-security: max-age=31536000' <<<"$headers"
check "and drops its Server header" bash -c '! grep -qi "^server:" <<<"$1"' _ "$headers"
check "the server's own security headers pass through" grep -qi '^x-content-type-options: nosniff' <<<"$headers"

token=$(sed -n 's/.*with the setup token \([^,]*\),.*/\1/p' "$work/server.log" | head -1)
check "the setup token is printed" test -n "$token"
c -o /dev/null -H 'Content-Type: application/json' -H "Origin: $base" \
  -d "{\"token\":\"$token\",\"username\":\"admin\",\"password\":\"a good password\"}" "$base/api/setup"
login=$(c -D - -H 'Content-Type: application/json' -H "Origin: $base" \
  -d '{"username":"admin","password":"a good password"}' "$base/api/login")
cookie=$(grep -i '^set-cookie: chessdb_session=' <<<"$login" | sed 's/^[^:]*: //; s/;.*//' | tr -d '\r')
csrf=$(grep -o '"csrfToken":"[^"]*"' <<<"$login" | cut -d'"' -f4)
check "the session cookie is Secure, HttpOnly, SameSite=Strict" \
  grep -qi '^set-cookie: chessdb_session=.*Secure.*' <<<"$(grep -i '^set-cookie' <<<"$login" | grep -i httponly | grep -i 'samesite=strict')"

post() { # post ORIGIN PATH JSON → status
  c -o /dev/null -w '%{http_code}' -H 'Content-Type: application/json' -H "Origin: $1" \
    -H "Cookie: $cookie" -H "X-CSRF-Token: $csrf" -d "$3" "$base$2"
}
check "a request from another origin is refused" test "$(post https://evil.example /api/databases '{"name":"x"}')" = 403
check "one from the public URL is taken" test "$(post "$base" /api/databases '{"name":"Games"}')" = 201
db=$(c -H "Cookie: $cookie" "$base/api/databases" | grep -o '"id":"[0-9a-f]\{32\}"' | head -1 | cut -d'"' -f4)

# An upload of a few MB, then the export streamed back compressed.
pgn="$work/games.pgn"
for i in $(seq 4000); do
  printf '[Event "Caddy"]\n[White "White %s"]\n[Black "Black %s"]\n[Result "1-0"]\n\n1. e4 e5 2. Nf3 Nc6 3. Bb5 a6 1-0\n\n' "$i" "$i"
done >"$pgn"
status=$(c -o "$work/up.json" -w '%{http_code}' -H "Origin: $base" -H "Cookie: $cookie" -H "X-CSRF-Token: $csrf" \
  --data-binary @"$pgn" "$base/api/databases/$db/import?filename=games.pgn")
check "an upload through the proxy starts an import" test "$status" = 202
job=$(grep -o '"job":[0-9]*' "$work/up.json" | cut -d: -f2)
for _ in $(seq 200); do
  c -H "Cookie: $cookie" "$base/api/jobs/$job" | grep -q '"state":"running"' || break
  sleep 0.1
done
check "the import is done" grep -q '"state":"done"' <(c -H "Cookie: $cookie" "$base/api/jobs/$job")
export_headers=$(c --compressed -D - -o "$work/out.pgn" -H "Cookie: $cookie" -H 'Accept-Encoding: gzip' \
  "$base/api/databases/$db/export.pgn")
check "the export comes compressed" grep -qi '^content-encoding: \(gzip\|zstd\)' <<<"$export_headers"
check "with every game" test "$(grep -c '^\[Event "Caddy"\]' "$work/out.pgn")" = 4000

# The desktop app's client (ureq, rustls with the system's certificates:
# here Caddy's local CA, through SSL_CERT_FILE) makes a copy through it.
cargo build -q -p chess-net --example server_copy
copy=$(SSL_CERT_FILE="$ca" CHESS_SERVER_PASSWORD="a good password" \
  target/debug/examples/server_copy "$base" admin "$db" "$work/copy.chessdb" 2>&1 || true)
check "the app's client copies the database over HTTPS ($copy)" grep -q '4000 new games' <<<"$copy"
copy=$(SSL_CERT_FILE="$ca" CHESS_SERVER_PASSWORD="a good password" \
  target/debug/examples/server_copy "$base" admin "$db" "$work/copy.chessdb" 2>&1 || true)
check "and updates it ($copy)" grep -q '0 new games, 0 changed, 4000 games' <<<"$copy"

# The client's address is the one Caddy saw, whatever the client claims:
# failed logins under five names, each claiming another address, are still
# one address's, and the sixth is throttled.
for i in 1 2 3 4 5; do
  c -o /dev/null -H 'Content-Type: application/json' -H "X-Forwarded-For: 10.0.0.$i" \
    -d "{\"username\":\"nobody$i\",\"password\":\"wrong password\"}" "$base/api/login"
done
status=$(c -o /dev/null -w '%{http_code}' -H 'Content-Type: application/json' -H 'X-Forwarded-For: 10.0.0.9' \
  -d '{"username":"nobody9","password":"wrong password"}' "$base/api/login")
check "a spoofed X-Forwarded-For doesn't get past the login throttle" test "$status" = 429
audit=$(c -H "Cookie: $cookie" "$base/api/admin/audit?action=login")
check "the audit log has the client's real address" grep -q '"client":"127.0.0.1"' <<<"$audit"
check "and none it claimed" bash -c '! grep -q "10\.0\.0\." <<<"$1"' _ "$audit"

if ((fails > 0)); then
  echo "$fails checks failed. Logs: server, caddy below"
  tail -n 20 "$work/server.log" "$work/caddy.log"
  exit 1
fi
echo "All checks passed behind Caddy $(caddy version | awk '{print $1}')."
