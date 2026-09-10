#!/usr/bin/env bash
# Reproducible release diagnostic for selected-ID-first conversion scaling.
set -euo pipefail

runs=${TARGETED_RUNS:-5}
total_signals=${TARGETED_TOTAL_SIGNALS:-128}
cycles=${TARGETED_CYCLES:-1000}
[[ $runs =~ ^[1-9][0-9]*$ ]] || { echo "TARGETED_RUNS must be positive" >&2; exit 2; }
[[ $total_signals =~ ^[1-9][0-9]*$ ]] || { echo "TARGETED_TOTAL_SIGNALS must be positive" >&2; exit 2; }
[[ $cycles =~ ^[1-9][0-9]*$ ]] || { echo "TARGETED_CYCLES must be positive" >&2; exit 2; }
command -v python3 >/dev/null || { echo "python3 is required" >&2; exit 2; }
[[ -x /usr/bin/time ]] || { echo "GNU /usr/bin/time is required" >&2; exit 2; }

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
printf 'schema\tvcd_targeted_conversion_v1\n'
printf 'git_commit\t%s\n' "$commit"
printf 'git_dirty\t%s\n' "$dirty"
printf 'rustc\t%s\n' "$(rustc -Vv | tr '\n' ';')"
printf 'uname\t%s\n' "$(uname -a)"
printf 'cpu\t%s\n' "$(LC_ALL=C lscpu 2>/dev/null | awk -F: '/Model name/{sub(/^[[:space:]]+/,"",$2); print $2; exit}' || true)"
printf 'generator\tquery::stream::tests::write_dense_fixture\n'
printf 'generator_total_signals\t%s\n' "$total_signals"
printf 'generator_cycles\t%s\n' "$cycles"
printf 'generator_changes\t%s\n' "$((total_signals * cycles))"
printf 'target_counts\t1,10,100\n'
printf 'runs_per_target_count\t%s\n' "$runs"
printf 'timing_scope\tinternal elapsed covers stream construction/admission and decoding; external elapsed also includes deterministic fixture generation and test harness\n'
printf 'allocation_scope\tnot measured; conversion counts only\n'
printf 'command_template\tTARGETED_TOTAL_SIGNALS=%s TARGETED_CYCLES=%s TARGETED_TARGETS=<1|10|100> /usr/bin/time <release-lib-test> --exact query::stream::tests::probe_targeted_conversion --ignored --nocapture\n' "$total_signals" "$cycles"
printf 'sample\ttargets\trun\texternal_elapsed_s\tuser_s\tsystem_s\tmax_rss_kib\tprobe_summary\n'

for targets in 1 10 100; do
  if (( targets > total_signals )); then
    echo "target count $targets exceeds total signals $total_signals" >&2
    exit 2
  fi
  for ((run=1; run<=runs; run++)); do
    metrics=$(mktemp "${TMPDIR:-/tmp}/vcd-targeted-time.XXXXXX")
    output=$(mktemp "${TMPDIR:-/tmp}/vcd-targeted-output.XXXXXX")
    cleanup() { rm -f "$metrics" "$output"; }
    trap cleanup EXIT
    TARGETED_TOTAL_SIGNALS=$total_signals \
    TARGETED_CYCLES=$cycles \
    TARGETED_TARGETS=$targets \
      /usr/bin/time -o "$metrics" -f '%e\t%U\t%S\t%M' \
      "$test_bin" --exact query::stream::tests::probe_targeted_conversion --ignored --nocapture \
      >"$output" 2>&1
    probe=$(grep 'targeted_conversion total_signals=' "$output" | tail -1 | tr '\t' ' ')
    [[ -n $probe ]] || { cat "$output" >&2; exit 3; }
    printf 'sample\t%s\t%s\t%s\t%s\n' "$targets" "$run" "$(cat "$metrics")" "$probe"
    cleanup
    trap - EXIT
  done
done
