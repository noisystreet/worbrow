#!/usr/bin/env bash
# Cyclomatic complexity gate via lizard (CCN). Shared by pre-commit and
# `make complexity`. Threshold is lizard's default (15); clippy already
# enforces cognitive_complexity at 10 on the same tree.
set -euo pipefail

CCN="${LIZARD_CCN:-15}"

if ! command -v lizard >/dev/null 2>&1; then
	echo "lizard not found. Install: python3 -m pip install 'lizard==1.22.1'" >&2
	echo "pre-commit installs it via additional_dependencies." >&2
	exit 1
fi

args=(-l rust -C "$CCN" -w)
if [[ $# -eq 0 ]]; then
	exec lizard "${args[@]}" src tests examples
fi

rs=()
for f in "$@"; do
	if [[ "$f" == *.rs ]]; then
		rs+=("$f")
	fi
done
if [[ ${#rs[@]} -eq 0 ]]; then
	exit 0
fi
exec lizard "${args[@]}" "${rs[@]}"
