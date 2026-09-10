#!/usr/bin/env bash
# Build the current PyO3 extension and run its compatibility suite.
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"

venv=${PYTHON_TEST_VENV:-$root/target/python-extension-venv}
base_python=${PYTHON:-python3}
if [[ ! -x $venv/bin/python ]]; then
  "$base_python" -m venv "$venv"
fi

export VIRTUAL_ENV=$venv
export PATH=$venv/bin:$PATH
if ! python -c 'import maturin, pytest' >/dev/null 2>&1; then
  python -m pip install 'maturin>=1.7,<2' pytest
fi

maturin develop --release
python -m pytest -q tests/python/test_extension_compat.py
