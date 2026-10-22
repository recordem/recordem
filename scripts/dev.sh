#!/usr/bin/env bash
#
# dev.sh — the dev environment in a tmux session (`recordem-dev`), one window per service.
#
#   scripts/dev.sh up [service...]        creates the missing windows and starts them (running ones are left alone)
#   scripts/dev.sh restart [service...]   kills and starts again
#   scripts/dev.sh stop [service...]      stops the process but keeps the window (pause)
#   scripts/dev.sh start [service...]     starts a stopped window again
#   scripts/dev.sh down [service...]      closes the windows (no service: the whole session)
#   scripts/dev.sh status                 what runs, on which port, and the link
#   scripts/dev.sh logs <service> [-f]    the service's log (in /tmp/recordem-dev/<service>.log)
#   scripts/dev.sh attach                 enters the session
#
# No service means all of them. Services: auth api worker web mcp.
# `up` first starts Postgres and Redis from compose.yaml (ports 39420 and 39421) when .env points
# at localhost, and applies the migrations; `status` shows both.
set -euo pipefail

SESSION=recordem-dev
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LOGS=/tmp/recordem-dev
HOST=${RECORDEM_DEV_HOST:-10.20.0.20}

SERVICES=(auth api worker web mcp)

command_of() {
  case "$1" in
    auth | api | worker | web | mcp) echo "direnv exec . bun run --filter @recordem/$1 dev" ;;
    *) echo "unknown service: $1 (use: ${SERVICES[*]})" >&2; exit 1 ;;
  esac
}

port() {
  case "$1" in
    auth) echo 39410 ;; api) echo 39411 ;; web) echo 39412 ;; mcp) echo 39414 ;; *) echo "" ;;
  esac
}

scheme() { [[ "$1" == web ]] && echo https || echo http; }

window() { echo "$SESSION:$1"; }
has_session() { tmux has-session -t "$SESSION" 2>/dev/null; }
has_window() { has_session && tmux list-windows -t "$SESSION" -F '#W' | grep -qx "$1"; }

# Through direnv, so .env and ~/.recordem-secrets load as in an interactive shell.
# The process runs with its log in a file; when it exits, the window stays open in a shell.
line() { echo "$(command_of "$1") 2>&1 | tee $LOGS/$1.log; echo; echo '■ $1 exited — scripts/dev.sh start $1'; exec \$SHELL"; }

launch() {
  local s=$1
  mkdir -p "$LOGS"
  if ! has_session; then
    tmux new-session -d -s "$SESSION" -n "$s" -c "$ROOT" "$(line "$s")"
  elif has_window "$s"; then
    tmux respawn-window -k -t "$(window "$s")" -c "$ROOT" "$(line "$s")"
  else
    tmux new-window -d -t "$SESSION" -n "$s" -c "$ROOT" "$(line "$s")"
  fi
  echo "▶ $s"
}

halt() {
  local s=$1
  has_window "$s" || { echo "· $s has no window"; return; }
  tmux respawn-window -k -t "$(window "$s")" -c "$ROOT" "echo '⏸ $s stopped — scripts/dev.sh start $s'; exec \$SHELL"
  echo "⏸ $s"
}

running() { # the window exists and its shell has children (stopped = just the waiting shell)
  has_window "$1" || return 1
  local pid
  pid=$(tmux display-message -p -t "$(window "$1")" '#{pane_pid}')
  pgrep -P "$pid" >/dev/null
}

# Local database and queue: without them auth and api fail at the first query.
infra() {
  if grep -qE '^(DATABASE_URL|REDIS_URL)=.*(localhost|127\.0\.0\.1)' "$ROOT/.env" 2>/dev/null; then
    docker compose -f "$ROOT/compose.yaml" up -d --wait postgres redis >/dev/null && echo "▶ postgres and redis (compose)"
  fi
  (cd "$ROOT" && bun run db:migrate >/dev/null 2>&1) && echo "▶ migrations" || echo "✕ migrations failed: bun run db:migrate"
}

targets() { if (($#)); then for s in "$@"; do command_of "$s" >/dev/null; echo "$s"; done; else printf '%s\n' "${SERVICES[@]}"; fi; }

action=${1:-status}
shift || true

case "$action" in
  up)
    (($#)) || infra
    for s in $(targets "$@"); do if running "$s"; then echo "· $s is already running"; else launch "$s"; fi; done ;;
  restart | start)
    for s in $(targets "$@"); do launch "$s"; done ;;
  stop)
    for s in $(targets "$@"); do halt "$s"; done ;;
  down)
    if (($#)); then
      for s in $(targets "$@"); do has_window "$s" && tmux kill-window -t "$(window "$s")" && echo "✕ $s"; done
    else
      has_session && tmux kill-session -t "$SESSION" && echo "✕ session $SESSION"
    fi ;;
  status)
    printf '%-10s %-9s %-6s %s\n' service state port link
    for s in "${SERVICES[@]}"; do
      p=$(port "$s")
      if running "$s"; then state=running; elif has_window "$s"; then state=stopped; else state=—; fi
      listening=""
      [[ -n "$p" ]] && ss -ltn "sport = :$p" | grep -q LISTEN && listening="$(scheme "$s")://$HOST:$p"
      [[ -n "$p" && -z "$listening" && "$state" == running ]] && listening="(starting…)"
      printf '%-10s %-9s %-6s %s\n' "$s" "$state" "${p:-—}" "$listening"
    done
    for c in postgres:39420 redis:39421; do
      ss -ltn "sport = :${c#*:}" | grep -q LISTEN && st=listening || st="stopped (scripts/dev.sh up)"
      printf '%-10s %-9s %-6s\n' "${c%%:*}" "$st" "${c#*:}"
    done ;;
  logs)
    s=${1:?usage: scripts/dev.sh logs <service> [-f]}
    command_of "$s" >/dev/null
    if [[ "${2:-}" == "-f" ]]; then tail -f "$LOGS/$s.log"; else tail -n 60 "$LOGS/$s.log"; fi ;;
  attach)
    exec tmux attach -t "$SESSION" ;;
  *)
    sed -n '3,16p' "$0"; exit 1 ;;
esac
