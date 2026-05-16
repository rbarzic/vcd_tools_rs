"""Command-line interface for vcd_tools — mirrors the vcd_tools_rs binary."""

import argparse
import json
import sys


def _load_signals_file(path):
    with open(path) as f:
        return [
            line.split("#")[0].strip()
            for line in f
            if line.split("#")[0].strip()
        ]


def _collect_signals(signal_args, signals_file):
    """Flatten repeated --signal args (supports comma-separated values) and optional file."""
    names = []
    for entry in signal_args:
        for part in entry.split(","):
            part = part.strip()
            if part:
                names.append(part)
    if signals_file:
        names.extend(_load_signals_file(signals_file))
    if not names:
        print("Error: at least one --signal or --signals-file entry is required", file=sys.stderr)
        sys.exit(1)
    return names


def _print_table(headers, rows):
    widths = [len(h) for h in headers]
    for row in rows:
        for i, cell in enumerate(row):
            if i < len(widths):
                widths[i] = max(widths[i], len(str(cell)))
    sep = "-+-".join("-" * w for w in widths)
    print(" | ".join(h.ljust(w) for h, w in zip(headers, widths)))
    print(sep)
    for row in rows:
        print(" | ".join(str(c).ljust(w) for c, w in zip(row, widths)))


def cmd_list(args):
    from vcd_tools import list_signals
    names = list_signals(args.vcd, args.filter)
    if args.pretty:
        _print_table(["Name"], [[n] for n in names])
    else:
        for n in names:
            print(n)


def cmd_meta(args):
    from vcd_tools import metadata
    m = metadata(args.vcd)
    ts = m.get("timescale") or "n/a"
    rows = [
        ("signals",    str(m["signal_count"])),
        ("timescale",  ts),
        ("start_time", str(m["start_time"])),
        ("end_time",   str(m["end_time"])),
    ]
    if args.pretty:
        _print_table(["Field", "Value"], rows)
    else:
        for k, v in rows:
            print(f"{k}\t{v}")


def cmd_extract(args):
    from vcd_tools import extract
    signals = _collect_signals(args.signal, args.signals_file)
    events = extract(args.vcd, signals=signals, start=args.start, end=args.end)

    # Build time-aligned rows (one row per timestamp, signals as columns)
    headers = ["time"] + signals
    aligned = {}  # time → {signal: value}
    times_seen = []
    for v in events:
        t = v["time"]
        if t not in aligned:
            aligned[t] = {}
            times_seen.append(t)
        aligned[t][v["signal"]] = v["value"]

    if args.pretty:
        rows = []
        for t in times_seen:
            row = [str(t)] + [aligned[t].get(s, "") for s in signals]
            rows.append(row)
        _print_table(headers, rows)
    else:
        print("\t".join(headers))
        for t in times_seen:
            row = [str(t)] + [aligned[t].get(s, "") for s in signals]
            print("\t".join(row))


def cmd_toggle(args):
    from vcd_tools import toggles
    signals = _collect_signals(args.signal, args.signals_file)
    counts = toggles(args.vcd, signals=signals, start=args.start, end=args.end)

    headers = ["signal", "toggles"]
    rows = [(s, str(counts.get(s, 0))) for s in signals]  # preserve order

    if args.pretty:
        _print_table(headers, rows)
    else:
        print("\t".join(headers))
        for row in rows:
            print("\t".join(row))


def cmd_find(args):
    from vcd_tools import find
    r = find(
        args.vcd,
        signal=args.signal,
        value=args.value,
        occurrence=args.occurrence,
        start=args.start,
        end=args.end,
    )
    headers = ["time", args.signal]
    if r["found"]:
        row = [str(r["time"]), str(r["value"])]
        if args.pretty:
            _print_table(headers, [row])
        else:
            print("\t".join(headers))
            print("\t".join(row))
    else:
        print(f"No matching occurrence found in the specified window.", file=sys.stderr)
        sys.exit(1)


def cmd_compare(args):
    from vcd_tools import compare
    signals_only = None
    if args.signals_only:
        signals_only = [s.strip() for s in args.signals_only.split(",") if s.strip()]
    result = compare(
        args.reference,
        args.actual,
        max_mismatches=args.max_mismatches,
        signals=signals_only,
        ignore_unknown=args.ignore_unknown,
        start=args.start,
        end=args.end,
    )

    fmt = args.output or "default"

    if fmt == "json":
        json_out = {k: v for k, v in result.items() if k != "mismatches"}
        # add per-signal mismatch counts
        by_signal = {}
        for m in result["mismatches"]:
            by_signal[m["signal"]] = by_signal.get(m["signal"], 0) + 1
        json_out["mismatches_by_signal"] = by_signal
        print(json.dumps(json_out, indent=2))
        return

    if fmt == "compact":
        n = len(result["common_signals"])
        if result["passed"]:
            print(f"✅ PASS: All {n} signals match")
        else:
            print(f"❌ FAIL: {result['signals_with_mismatches']}/{n} signals have mismatches "
                  f"({result['total_mismatches']} total)")
        return

    # default — detailed
    print("==========================================")
    print("VCD Comparison Results")
    print("==========================================")
    print()
    print(f"Reference: {result['file1']}")
    print(f"Actual:    {result['file2']}")
    print()

    if result["signals_only_in_file1"]:
        print("Signals only in reference:")
        for s in result["signals_only_in_file1"]:
            print(f"  - {s}")
        print()
    if result["signals_only_in_file2"]:
        print("Signals only in actual:")
        for s in result["signals_only_in_file2"]:
            print(f"  - {s}")
        print()

    n = len(result["common_signals"])
    print(f"Common signals: {n}")
    print()

    if not n:
        print("⚠️  No common signals to compare!")
        return

    print("==========================================")
    print("Signal Value Comparison")
    print("==========================================")
    print()

    if result["passed"]:
        print(f"All common signals match ({n} total).")
        print()
    else:
        by_signal = {}
        for m in result["mismatches"]:
            by_signal.setdefault(m["signal"], []).append(m)
        for sig in sorted(by_signal):
            mismatches = by_signal[sig]
            print(f"Signal: {sig}")
            for mm in mismatches[:10]:
                status = "(one or both unknown)" if mm["is_unknown"] else "❌ MISMATCH"
                print(f"  Time #{mm['time']}: Ref='{mm['value1']}' | Actual='{mm['value2']}' {status}")
            if len(mismatches) > 10:
                print(f"  ... and {len(mismatches) - 10} more")
            print(f"  ❌ {len(mismatches)} mismatches")
            print()
        matched = n - result["signals_with_mismatches"]
        print(f"Matched signals: {matched} / {n}")
        print()

    print("==========================================")
    print("Summary")
    print("==========================================")
    print()
    if result["passed"]:
        print("✅ SUCCESS: All signal values match!")
        print()
        print("The two VCD files are equivalent.")
    else:
        print("❌ FAILURES FOUND")
        print()
        print(f"Total mismatches: {result['total_mismatches']}")
        print(f"Signals with mismatches: {result['signals_with_mismatches']} / {n}")
    print()


def main():
    parser = argparse.ArgumentParser(
        prog="vcd_tools_rs",
        description="VCD file analysis tools",
    )
    parser.add_argument("--version", action="version", version="%(prog)s 0.1.7")
    parser.add_argument("--log-level",
                        default="info",
                        choices=["error", "warn", "info", "debug"],
                        help="Logging level (default: info)")
    parser.add_argument("--pretty",
                        action="store_true",
                        help="Render output using tables")
    sub = parser.add_subparsers(dest="command", required=True)

    # list
    p = sub.add_parser("list", help="List signals declared in the VCD header")
    p.add_argument("vcd")
    p.add_argument("--filter", help="Substring filter applied to signal names")

    # meta
    p = sub.add_parser("meta", help="Show metadata for a VCD file")
    p.add_argument("vcd")

    # extract
    p = sub.add_parser("extract", help="Extract time/value pairs for specific signals")
    p.add_argument("vcd")
    p.add_argument("--signal", action="append", default=[], metavar="SIGNAL",
                   help="Signal name to extract (repeatable, comma-separated ok)")
    p.add_argument("--signals-file", help="File with one signal name per line")
    p.add_argument("--start", type=int, help="Start time (inclusive)")
    p.add_argument("--end", type=int, help="End time (inclusive)")

    # toggle
    p = sub.add_parser("toggle", help="Count value transitions per signal")
    p.add_argument("vcd")
    p.add_argument("--signal", action="append", default=[], metavar="SIGNAL",
                   help="Signal name (repeatable, comma-separated ok)")
    p.add_argument("--signals-file", help="File with one signal name per line")
    p.add_argument("--start", type=int, help="Start time (inclusive)")
    p.add_argument("--end", type=int, help="End time (inclusive)")

    # find
    p = sub.add_parser("find", help="Find Nth occurrence of a signal value")
    p.add_argument("vcd")
    p.add_argument("--signal", required=True, help="Signal to watch")
    p.add_argument("--value", required=True, help="Target value (decimal, 0x hex, x, z)")
    p.add_argument("--occurrence", "--occurence",  # match Rust typo alias
                   type=int, default=1, dest="occurrence",
                   help="Which occurrence to return (default: 1)")
    p.add_argument("--start", type=int)
    p.add_argument("--end", type=int)

    # compare
    p = sub.add_parser("compare", help="Compare two VCD files and report differences")
    p.add_argument("reference")
    p.add_argument("actual")
    p.add_argument("--max-mismatches", type=int, help="Limit number of mismatches per signal")
    p.add_argument("--signals-only", help="Only compare specific signals (comma-separated)")
    p.add_argument("--ignore-unknown", action="store_true",
                   help="Ignore x/z differences when comparing")
    p.add_argument("--start", type=int)
    p.add_argument("--end", type=int)
    p.add_argument("--output", choices=["default", "json", "compact"],
                   help="Output format (default, json, compact)")

    args = parser.parse_args()

    dispatch = {
        "list": cmd_list,
        "meta": cmd_meta,
        "extract": cmd_extract,
        "toggle": cmd_toggle,
        "find": cmd_find,
        "compare": cmd_compare,
    }
    try:
        dispatch[args.command](args)
    except Exception as e:
        print(f"Error: {e}", file=sys.stderr)
        sys.exit(1)


if __name__ == "__main__":
    main()
