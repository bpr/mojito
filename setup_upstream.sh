#!/usr/bin/env bash
# Run on the NEW machine: recreate ~/src/mojo/repos/modular and the pinned
# worktree ~/src/mojo/repos/modular-e6e596ce exactly as on the old machine.
set -euo pipefail

PIN=e6e596ce2cdd94ae36b68ec604cba2e2927370a3     # Mojo 1.3.0.dev2026111316 lockfiles
MAIN=e6e596ce2cdd94ae36b68ec604cba2e2927370a3    # old machine's main checkout

mkdir -p ~/src/mojo/repos
cd ~/src/mojo/repos

if [ ! -d modular/.git ]; then
    git clone https://github.com/modular/modular.git modular   # ~1.1 GB
fi
cd modular
git fetch origin
git checkout main
git reset --hard "$MAIN"
git branch --set-upstream-to=origin/main main

if [ ! -d ../modular-e6e596ce ]; then
    git worktree add --detach ../modular-e6e596ce "$PIN"
fi

echo
git -C ~/src/mojo/repos/modular log -1 --format='main:   %h %s'
git -C ~/src/mojo/repos/modular-e6e596ce log -1 --format='pinned: %h %s'
ls ~/src/mojo/repos/modular-e6e596ce/Mojo/stdlib/std/simd.mojo
