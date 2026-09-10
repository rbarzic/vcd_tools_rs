#!/usr/bin/env bash
# Reproducible release-binary benchmark for one vcd_tools_rs operation.
set -euo pipefail

usage() {
  cat >&2 <<'USAGE'
Usage: scripts/bench-vcd.sh VCD OPERATION [OPERATION ARGUMENTS...]

Operations:
  open                         Parse/catalog and emit the complete signal list
  list [list options]
  meta
  extract [extract options]
  find [find options]
  toggle [toggle options]
  compare ACTUAL_VCD [compare options]

Environment:
  VCD_TOOLS_BIN                Binary (default: ./target/release/vcd_tools_rs)
  BENCH_RUNS                   Measured runs (default: 1)
  BENCH_ARTIFACT_DIR           Persist TSV record and per-run stdout/stderr
  BENCH_LABEL                  Artifact filename prefix
  BENCH_KEEP_OUTPUT=1          Keep temporary output when no artifact dir is used
  BENCH_EXPECT_SHA256          Require every stdout SHA-256 to match
  BENCH_EXPECT_VCD_SHA256      Require primary input SHA-256 to match
  BENCH_EXPECT_SECONDARY_SHA256 Require compare secondary SHA-256 to match

Every input VCD is fingerprinted with a complete SHA-256 plus file metadata.
Every run records complete stdout/stderr hashes. The command exits nonzero on a
command or expected-hash mismatch. It does not drop the OS page cache.
USAGE
  exit 2
}

[[ $# -ge 2 ]] || usage
vcd=$1
operation=$2
shift 2

[[ -r "$vcd" ]] || { echo "VCD is not readable: $vcd" >&2; exit 2; }

bin=${VCD_TOOLS_BIN:-./target/release/vcd_tools_rs}
runs=${BENCH_RUNS:-1}
[[ -x "$bin" ]] || { echo "Benchmark binary is not executable: $bin" >&2; exit 2; }
[[ "$runs" =~ ^[1-9][0-9]*$ ]] || { echo "BENCH_RUNS must be a positive integer" >&2; exit 2; }
[[ -x /usr/bin/time ]] || { echo "GNU /usr/bin/time is required" >&2; exit 2; }
command -v sha256sum >/dev/null || { echo "sha256sum is required" >&2; exit 2; }

secondary=
case "$operation" in
  open)
    [[ $# -eq 0 ]] || usage
    command_args=(list "$vcd")
    ;;
  list|extract|find|toggle)
    command_args=("$operation" "$vcd" "$@")
    ;;
  meta)
    [[ $# -eq 0 ]] || usage
    command_args=(meta "$vcd")
    ;;
  compare)
    [[ $# -ge 1 ]] || usage
    secondary=$1
    shift
    [[ -r "$secondary" ]] || { echo "Secondary VCD is not readable: $secondary" >&2; exit 2; }
    command_args=(compare "$vcd" "$secondary" "$@")
    ;;
  *) usage ;;
esac

primary_sha=$(sha256sum "$vcd" | awk '{print $1}')
if [[ -n ${BENCH_EXPECT_VCD_SHA256:-} && $primary_sha != "$BENCH_EXPECT_VCD_SHA256" ]]; then
  echo "Primary VCD SHA-256 mismatch: expected $BENCH_EXPECT_VCD_SHA256, got $primary_sha" >&2
  exit 3
fi

secondary_sha=
if [[ -n $secondary ]]; then
  if [[ $(readlink -f "$secondary") == $(readlink -f "$vcd") ]]; then
    secondary_sha=$primary_sha
  else
    secondary_sha=$(sha256sum "$secondary" | awk '{print $1}')
  fi
  if [[ -n ${BENCH_EXPECT_SECONDARY_SHA256:-} && $secondary_sha != "$BENCH_EXPECT_SECONDARY_SHA256" ]]; then
    echo "Secondary VCD SHA-256 mismatch: expected $BENCH_EXPECT_SECONDARY_SHA256, got $secondary_sha" >&2
    exit 3
  fi
fi

artifact_record=
artifact_label=
if [[ -n ${BENCH_ARTIFACT_DIR:-} ]]; then
  mkdir -p "$BENCH_ARTIFACT_DIR"
  artifact_label=${BENCH_LABEL:-$(basename "$vcd").${operation}.$(date -u +%Y%m%dT%H%M%SZ).$$}
  [[ $artifact_label =~ ^[A-Za-z0-9._-]+$ ]] || {
    echo "BENCH_LABEL may contain only letters, digits, dot, underscore, and hyphen" >&2
    exit 2
  }
  artifact_record=$BENCH_ARTIFACT_DIR/${artifact_label}.tsv
  exec > >(tee "$artifact_record")
fi

command_text=$(printf '%q ' "$bin" "${command_args[@]}")
commit=$(git rev-parse HEAD 2>/dev/null || printf unknown)
if ! git diff --quiet --ignore-submodules HEAD 2>/dev/null ||
   [[ -n $(git status --porcelain --untracked-files=normal 2>/dev/null) ]]; then
  dirty=true
else
  dirty=false
fi

filesystem=$(findmnt -T "$vcd" -n -o FSTYPE,SOURCE,TARGET 2>/dev/null | tr '\t' ' ' || true)
storage=$(lsblk -dno NAME,TYPE,ROTA,MODEL 2>/dev/null | tr '\n' ';' || true)

printf 'record\tvalue\n'
printf 'schema\tvcd_tools_benchmark_v2\n'
printf 'artifact_record\t%s\n' "${artifact_record:-not-persisted-by-script}"
printf 'binary\t%s\n' "$bin"
printf 'binary_version\t%s\n' "$($bin --version | tr '\t\n' '  ')"
printf 'git_commit\t%s\n' "$commit"
printf 'git_dirty\t%s\n' "$dirty"
printf 'rustc\t%s\n' "$(rustc -Vv | tr '\n' ';')"
printf 'cargo\t%s\n' "$(cargo -V)"
printf 'uname\t%s\n' "$(uname -a)"
printf 'cpu\t%s\n' "$(LC_ALL=C lscpu 2>/dev/null | awk -F: '/Model name/{sub(/^[[:space:]]+/,"",$2); print $2; exit}' || true)"
printf 'memory_kib\t%s\n' "$(awk '/MemTotal/{print $2}' /proc/meminfo 2>/dev/null || printf unknown)"
printf 'filesystem\t%s\n' "$filesystem"
printf 'storage_devices\t%s\n' "$storage"
printf 'primary_vcd_path\t%s\n' "$vcd"
printf 'primary_vcd_size_bytes\t%s\n' "$(stat -c %s "$vcd")"
printf 'primary_vcd_mtime\t%s\n' "$(stat -c %y "$vcd")"
printf 'primary_vcd_sha256\t%s\n' "$primary_sha"
printf 'primary_vcd_hash_verified\t%s\n' "$([[ -n ${BENCH_EXPECT_VCD_SHA256:-} ]] && echo true || echo not-requested)"
if [[ -n $secondary ]]; then
  printf 'secondary_vcd_path\t%s\n' "$secondary"
  printf 'secondary_vcd_size_bytes\t%s\n' "$(stat -c %s "$secondary")"
  printf 'secondary_vcd_mtime\t%s\n' "$(stat -c %y "$secondary")"
  printf 'secondary_vcd_sha256\t%s\n' "$secondary_sha"
  printf 'secondary_vcd_hash_verified\t%s\n' "$([[ -n ${BENCH_EXPECT_SECONDARY_SHA256:-} ]] && echo true || echo not-requested)"
fi
printf 'operation\t%s\n' "$operation"
printf 'command\t%s\n' "$command_text"
printf 'cache_policy\tuncontrolled; page cache not dropped\n'

for ((run=1; run<=runs; run++)); do
  output=$(mktemp "${TMPDIR:-/tmp}/vcd-tools-bench-output.XXXXXX")
  error=$(mktemp "${TMPDIR:-/tmp}/vcd-tools-bench-error.XXXXXX")
  metrics=$(mktemp "${TMPDIR:-/tmp}/vcd-tools-bench-time.XXXXXX")
  cleanup() {
    rm -f "$error" "$metrics"
    if [[ ${BENCH_KEEP_OUTPUT:-0} != 1 && -z ${BENCH_ARTIFACT_DIR:-} ]]; then rm -f "$output"; fi
  }
  trap cleanup EXIT

  set +e
  LC_ALL=C /usr/bin/time -o "$metrics" \
    -f 'elapsed_s=%e\tuser_s=%U\tsystem_s=%S\tmax_rss_kib=%M\tfs_inputs=%I\tfs_outputs=%O\texit=%x' \
    "$bin" "${command_args[@]}" >"$output" 2>"$error"
  status=$?
  set -e

  output_bytes=$(stat -c %s "$output")
  output_sha=$(sha256sum "$output" | awk '{print $1}')
  error_bytes=$(stat -c %s "$error")
  error_sha=$(sha256sum "$error" | awk '{print $1}')

  printf 'run_%d_timing\t%s\n' "$run" "$(cat "$metrics")"
  printf 'run_%d_output_bytes\t%s\n' "$run" "$output_bytes"
  printf 'run_%d_output_sha256\t%s\n' "$run" "$output_sha"
  printf 'run_%d_stderr_bytes\t%s\n' "$run" "$error_bytes"
  printf 'run_%d_stderr_sha256\t%s\n' "$run" "$error_sha"
  printf 'run_%d_status\t%s\n' "$run" "$status"

  if [[ -n ${BENCH_EXPECT_SHA256:-} ]]; then
    if [[ $output_sha == "$BENCH_EXPECT_SHA256" ]]; then
      printf 'run_%d_output_hash_verified\ttrue\n' "$run"
    else
      printf 'run_%d_output_hash_verified\tfalse\n' "$run"
      echo "Output SHA-256 mismatch on run $run: expected $BENCH_EXPECT_SHA256, got $output_sha" >&2
      exit 3
    fi
  else
    printf 'run_%d_output_hash_verified\tnot-requested\n' "$run"
  fi

  if [[ $operation == open && $output_bytes -eq 0 ]]; then
    echo "Open/catalog benchmark produced an empty signal catalog" >&2
    exit 3
  fi
  if [[ $status -ne 0 ]]; then
    echo "Benchmark command failed on run $run:" >&2
    cat "$error" >&2
    exit "$status"
  fi

  if [[ -n ${BENCH_ARTIFACT_DIR:-} ]]; then
    output_artifact=$BENCH_ARTIFACT_DIR/${artifact_label}.run${run}.stdout
    error_artifact=$BENCH_ARTIFACT_DIR/${artifact_label}.run${run}.stderr
    cp "$output" "$output_artifact"
    cp "$error" "$error_artifact"
    printf 'run_%d_output_artifact\t%s\n' "$run" "$output_artifact"
    printf 'run_%d_stderr_artifact\t%s\n' "$run" "$error_artifact"
    rm -f "$output"
  elif [[ ${BENCH_KEEP_OUTPUT:-0} == 1 ]]; then
    printf 'run_%d_output_path\t%s\n' "$run" "$output"
  else
    rm -f "$output"
  fi

  rm -f "$error" "$metrics"
  trap - EXIT
done
