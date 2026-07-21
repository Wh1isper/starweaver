#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "usage: $0 /path/to/starweaver-desktop" >&2
  exit 2
fi

binary=$1
if [[ ! -x "$binary" ]]; then
  echo "desktop binary is not executable: $binary" >&2
  exit 2
fi
binary=$(cd "$(dirname "$binary")" && pwd)/$(basename "$binary")

workdir=$(mktemp -d)
primary_pid=
candidate_a_pid=
candidate_b_pid=
secondary_pid=
cleanup() {
  for pid in "$secondary_pid" "$candidate_a_pid" "$candidate_b_pid" "$primary_pid"; do
    if [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null; then
      kill "$pid" 2>/dev/null || true
      wait "$pid" 2>/dev/null || true
    fi
  done
  rm -rf "$workdir"
}
trap cleanup EXIT

process_running() {
  local state
  state=$(ps -p "$1" -o state= 2>/dev/null | tr -d ' ')
  [[ -n "$state" && "$state" != Z* ]]
}

socket_path="${TMPDIR:-/tmp}/starweaver-desktop-$(id -u)/activation-v1.sock"
if [[ -S "$socket_path" ]]; then
  if lsof "$socket_path" >/dev/null 2>&1; then
    echo "another desktop process already owns the activation socket" >&2
    exit 1
  fi
  rm -f "$socket_path"
fi

(
  while [[ ! -f "$workdir/start-cold-race" ]]; do sleep 0.01; done
  exec "$binary" >"$workdir/candidate-a.log" 2>&1
) &
candidate_a_pid=$!
(
  while [[ ! -f "$workdir/start-cold-race" ]]; do sleep 0.01; done
  exec "$binary" >"$workdir/candidate-b.log" 2>&1
) &
candidate_b_pid=$!
touch "$workdir/start-cold-race"
for _ in {1..100}; do
  a_running=false
  b_running=false
  if process_running "$candidate_a_pid"; then a_running=true; fi
  if process_running "$candidate_b_pid"; then b_running=true; fi

  if [[ "$a_running" == false && "$b_running" == true ]]; then
    set +e
    wait "$candidate_a_pid"
    candidate_status=$?
    set -e
    candidate_a_pid=
    primary_pid=$candidate_b_pid
    candidate_b_pid=
    break
  fi
  if [[ "$a_running" == true && "$b_running" == false ]]; then
    set +e
    wait "$candidate_b_pid"
    candidate_status=$?
    set -e
    candidate_b_pid=
    primary_pid=$candidate_a_pid
    candidate_a_pid=
    break
  fi
  if [[ "$a_running" == false && "$b_running" == false ]]; then
    cat "$workdir/candidate-a.log" "$workdir/candidate-b.log" >&2
    echo "both cold-start candidates exited" >&2
    exit 1
  fi
  sleep 0.1
done
if [[ -z "$primary_pid" ]]; then
  echo "concurrent cold start left more than one primary" >&2
  exit 1
fi
if [[ $candidate_status -ne 0 ]]; then
  cat "$workdir/candidate-a.log" "$workdir/candidate-b.log" >&2
  echo "secondary cold-start candidate exited with an error" >&2
  exit 1
fi
if [[ ! -S "$socket_path" ]] || ! process_running "$primary_pid"; then
  cat "$workdir/candidate-a.log" "$workdir/candidate-b.log" >&2
  echo "elected primary is not serving the activation socket" >&2
  exit 1
fi

(
  cd "$workdir"
  exec "$binary" --private-smoke-argument >"$workdir/secondary.log" 2>&1
) &
secondary_pid=$!
timeout_marker="$workdir/secondary-timeout"
(
  sleep 5
  if kill -0 "$secondary_pid" 2>/dev/null; then
    touch "$timeout_marker"
    kill "$secondary_pid" 2>/dev/null || true
  fi
) &
watchdog_pid=$!
set +e
wait "$secondary_pid"
secondary_status=$?
set -e
secondary_pid=
kill "$watchdog_pid" 2>/dev/null || true
wait "$watchdog_pid" 2>/dev/null || true
if [[ -f "$timeout_marker" ]]; then
  cat "$workdir/secondary.log" >&2
  echo "secondary desktop did not receive an activation acknowledgement" >&2
  exit 1
fi
if [[ $secondary_status -ne 0 ]]; then
  cat "$workdir/secondary.log" >&2
  echo "secondary desktop exited with an error" >&2
  exit 1
fi
if ! process_running "$primary_pid"; then
  cat "$workdir/candidate-a.log" "$workdir/candidate-b.log" >&2
  echo "primary desktop exited during secondary activation" >&2
  exit 1
fi
