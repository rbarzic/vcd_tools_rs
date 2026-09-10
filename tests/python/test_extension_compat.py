"""Compatibility tests for the compiled PyO3 extension.

Run through ``scripts/test-python-extension.sh`` so the current checkout is
built and installed into an isolated virtual environment before collection.
"""

from pathlib import Path
import inspect

import pytest
import vcd_tools


ROOT = Path(__file__).resolve().parents[2]
SEMANTICS = str(ROOT / "tests" / "fixtures" / "query_semantics.vcd")
CHANGED = str(ROOT / "tests" / "fixtures" / "query_semantics_changed.vcd")


def test_exported_module_functions_and_signatures_are_stable():
    assert vcd_tools.__all__ == [
        "compare",
        "extract",
        "find",
        "list_signals",
        "metadata",
        "toggles",
    ]
    assert str(inspect.signature(vcd_tools.list_signals)) == "(path, filter=None)"
    assert str(inspect.signature(vcd_tools.metadata)) == "(path)"
    assert str(inspect.signature(vcd_tools.extract)) == "(path, signals, start=None, end=None)"
    assert str(inspect.signature(vcd_tools.toggles)) == "(path, signals, start=None, end=None)"
    assert str(inspect.signature(vcd_tools.find)) == (
        "(path, signal, value, occurrence=1, start=None, end=None)"
    )
    assert str(inspect.signature(vcd_tools.compare)) == (
        "(file1, file2, *, max_mismatches=None, signals=None, "
        "ignore_unknown=False, start=None, end=None)"
    )


def test_list_and_metadata_shapes_and_values_are_stable():
    assert vcd_tools.list_signals(SEMANTICS) == [
        "top.a",
        "top.alias_a",
        "top.b",
        "top.vec4[3:0]",
        "top.wide[128:0]",
        "top.real_sig",
        "top.str_sig",
    ]
    assert vcd_tools.list_signals(SEMANTICS, filter="vec") == ["top.vec4[3:0]"]
    assert vcd_tools.metadata(SEMANTICS) == {
        "signal_count": 7,
        "start_time": 0,
        "end_time": 25,
        "timescale": "1 ns",
    }


def test_extract_toggle_and_find_shapes_and_semantics_are_stable():
    assert vcd_tools.extract(
        SEMANTICS,
        ["top.alias_a", "top.a"],
        start=5,
        end=5,
    ) == [
        {"signal": "top.alias_a", "time": 5, "value": "1"},
        {"signal": "top.a", "time": 5, "value": "1"},
    ]
    assert vcd_tools.toggles(SEMANTICS, ["top.a"], start=5, end=20) == {
        "top.a": 3
    }
    assert vcd_tools.find(SEMANTICS, "top.a", "1", occurrence=2) == {
        "found": True,
        "signal": "top.a",
        "time": 15,
        "value": "1",
    }
    assert vcd_tools.find(SEMANTICS, "top.a", "not-there") == {
        "found": False,
        "signal": "top.a",
        "time": None,
        "value": None,
    }


def test_compare_dictionary_shape_is_stable():
    result = vcd_tools.compare(SEMANTICS, CHANGED, signals=["top.a"])
    assert result == {
        "passed": False,
        "file1": SEMANTICS,
        "file2": CHANGED,
        "common_signals": [
            "top.a",
            "top.alias_a",
            "top.b",
            "top.real_sig",
            "top.str_sig",
            "top.vec4[3:0]",
            "top.wide[128:0]",
        ],
        "signals_only_in_file1": [],
        "signals_only_in_file2": [],
        "total_mismatches": 1,
        "signals_with_mismatches": 1,
        "mismatches": [
            {
                "signal": "top.a",
                "time": 15,
                "value1": "1",
                "value2": "0",
                "is_unknown": False,
            }
        ],
    }


def test_vcd_errors_remain_runtime_errors_with_stable_messages():
    with pytest.raises(RuntimeError, match=r"^signals not found in VCD: top\.missing$"):
        vcd_tools.extract(SEMANTICS, ["top.missing"])
    with pytest.raises(RuntimeError, match=r"^occurrence must be >= 1$"):
        vcd_tools.find(SEMANTICS, "top.a", "1", occurrence=0)
