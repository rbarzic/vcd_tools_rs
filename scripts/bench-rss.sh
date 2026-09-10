#!/usr/bin/env bash
# Convenience wrapper retaining the historical plan name. All resource metrics,
# including peak RSS, are emitted by bench-vcd.sh.
set -euo pipefail

if [[ $# -lt 2 ]]; then
  echo "Usage: scripts/bench-rss.sh VCD OPERATION [OPERATION ARGUMENTS...]" >&2
  exit 2
fi

BENCH_RUNS=${BENCH_RUNS:-1} exec "$(dirname "$0")/bench-vcd.sh" "$@"
