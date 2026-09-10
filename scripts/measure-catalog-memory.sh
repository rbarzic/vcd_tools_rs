#!/usr/bin/env bash
# Measure release-test-process peak RSS for compact-only and compatibility catalogs.
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "Usage: scripts/measure-catalog-memory.sh VCD" >&2
  exit 2
fi
vcd=$1
[[ -r $vcd ]] || { echo "VCD is not readable: $vcd" >&2; exit 2; }
[[ -x /usr/bin/time ]] || { echo "GNU /usr/bin/time is required" >&2; exit 2; }

command -v python3 >/dev/null || { echo "python3 is required" >&2; exit 2; }
command -v sha256sum >/dev/null || { echo "sha256sum is required" >&2; exit 2; }

runs=${CATALOG_RUNS:-4}
[[ $runs =~ ^[1-9][0-9]*$ ]] || { echo "CATALOG_RUNS must be a positive integer" >&2; exit 2; }

test_bin=$(
  cargo test --release --lib --no-run --message-format=json \
    | python3 -c '
import json, sys
for line in sys.stdin:
    try:
        message = json.loads(line)
    except json.JSONDecodeError:
        continue
    target = message.get("target", {})
    executable = message.get("executable")
    profile = message.get("profile", {})
    if (
        message.get("reason") == "compiler-artifact"
        and target.get("name") == "vcd_tools_rs"
        and target.get("src_path", "").endswith("/src/lib.rs")
        and profile.get("test")
        and executable
    ):
        print(executable)
'
)
test_bin=$(printf '%s\n' "$test_bin" | tail -1)
[[ -n $test_bin && -x $test_bin ]] || { echo "release lib test binary not found" >&2; exit 2; }

commit=$(git rev-parse HEAD 2>/dev/null || printf unknown)
if ! git diff --quiet --ignore-submodules HEAD 2>/dev/null ||
   [[ -n $(git status --porcelain --untracked-files=normal 2>/dev/null) ]]; then
  dirty=true
else
  dirty=false
fi

printf 'record\tvalue\n'
printf 'schema\tvcd_catalog_memory_v1\n'
printf 'git_commit\t%s\n' "$commit"
printf 'git_dirty\t%s\n' "$dirty"
printf 'rustc\t%s\n' "$(rustc -Vv | tr '\n' ';')"
printf 'uname\t%s\n' "$(uname -a)"
printf 'cpu\t%s\n' "$(LC_ALL=C lscpu 2>/dev/null | awk -F: '/Model name/{sub(/^[[:space:]]+/,"",$2); print $2; exit}' || true)"
printf 'memory_kib\t%s\n' "$(awk '/MemTotal/{print $2}' /proc/meminfo 2>/dev/null || printf unknown)"
printf 'filesystem\t%s\n' "$(findmnt -T "$vcd" -n -o FSTYPE,SOURCE,TARGET 2>/dev/null | tr '\t' ' ' || true)"
printf 'vcd_path\t%s\n' "$vcd"
printf 'vcd_size_bytes\t%s\n' "$(stat -c %s "$vcd")"
printf 'vcd_mtime\t%s\n' "$(stat -c %y "$vcd")"
printf 'vcd_sha256\t%s\n' "$(sha256sum "$vcd" | awk '{print $1}')"
printf 'cache_policy\tuncontrolled; page cache not dropped; compact and compatibility alternate per run\n'
printf 'runs_per_mode\t%s\n' "$runs"
printf 'test_binary\t%s\n' "$test_bin"
printf 'command_template\tVCD_CATALOG_PROBE=<vcd> /usr/bin/time <test-binary> --exact catalog::tests::probe_<mode>_peak_rss --ignored --nocapture\n'
printf 'sample\tmode\trun\telapsed_s\tuser_s\tsystem_s\tmax_rss_kib\tprobe_summary\n'

for ((run=1; run<=runs; run++)); do
  for mode in compact compatibility; do
    metrics=$(mktemp "${TMPDIR:-/tmp}/vcd-catalog-memory.XXXXXX")
    output=$(mktemp "${TMPDIR:-/tmp}/vcd-catalog-output.XXXXXX")
    cleanup() { rm -f "$metrics" "$output"; }
    trap cleanup EXIT
    VCD_CATALOG_PROBE=$vcd /usr/bin/time -o "$metrics" \
      -f '%e\t%U\t%S\t%M' \
      "$test_bin" --exact "catalog::tests::probe_${mode}_peak_rss" --ignored --nocapture \
      >"$output" 2>&1
    probe=$(grep "mode=$mode " "$output" | tail -1 | tr '\t' ' ')
    printf 'sample\t%s\t%d\t%s\t%s\n' "$mode" "$run" "$(cat "$metrics")" "$probe"
    cleanup
    trap - EXIT
  done
done
