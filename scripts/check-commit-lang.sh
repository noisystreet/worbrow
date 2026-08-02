#!/usr/bin/env bash
# Enforce English-only commit messages (no CJK), consistent with the i18n
# convention (AGENTS.md "Commit conventions"). Used as a pre-commit local hook
# at the commit-msg stage; receives the commit message file path as $1.
set -euo pipefail

msg_file="${1:?commit message file path required}"

if grep -qP '[\p{Han}]' "$msg_file"; then
    echo "commit message must be written in English (CJK characters detected):" >&2
    grep -nP '[\p{Han}]' "$msg_file" >&2
    exit 1
fi
exit 0
