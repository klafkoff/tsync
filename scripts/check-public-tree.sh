#!/bin/sh
# Fail the build if this tree looks like an operator's machine, not the product.
# Extra strings can live in .tsync-forbidden.local (gitignored). Do not put
# real hostnames in this script — that would publish them.

set -eu

root="$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)"
cd "$root"
fail=0

if [ -e deploy ] || [ -d deploy ]; then
  echo "check-public-tree: deploy/ must not exist in this repository" >&2
  fail=1
fi

# Tracked env files are a leak even when empty. .env.example is the exception.
if git ls-files | grep -E '(^|/)\.env($|\.)' | grep -v '\.env\.example$'; then
  echo "check-public-tree: tracked .env files are forbidden" >&2
  fail=1
fi

if [ -f .tsync-forbidden.local ]; then
  while IFS= read -r pattern || [ -n "$pattern" ]; do
    case "$pattern" in
      '' | \#*) continue ;;
    esac
    if git grep -n --fixed-string -- "$pattern" -- . ':!.tsync-forbidden.local' \
      >/dev/null 2>&1; then
      echo "check-public-tree: forbidden local pattern matched: $pattern" >&2
      git grep -n --fixed-string -- "$pattern" -- . ':!.tsync-forbidden.local' >&2 || true
      fail=1
    fi
  done < .tsync-forbidden.local
fi

if [ "$fail" -ne 0 ]; then
  exit 1
fi

echo "check-public-tree: ok"
