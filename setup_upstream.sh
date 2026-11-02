#!/usr/bin/env bash
# Run on the NEW machine: recreate ~/src/mojo/repos/modular and the pinned
# worktree ~/src/mojo/repos/modular-a431ffa8 exactly as on the old machine.
set -euo pipefail

PIN=a431ffa8af8ef7cb5bb4a11ec6409fed203107bb     # Mojo 1.3.0.dev2026110105 lockfiles
MAIN=8ed65fc6ac15950ce62da1f8e8d50c2c7c528065    # old machine's main checkout

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

if [ ! -d ../modular-a431ffa8 ]; then
    git worktree add --detach ../modular-a431ffa8 "$PIN"
fi

echo
git -C ~/src/mojo/repos/modular log -1 --format='main:   %h %s'
git -C ~/src/mojo/repos/modular-a431ffa8 log -1 --format='pinned: %h %s'
ls ~/src/mojo/repos/modular-a431ffa8/Mojo/stdlib/std/simd.mojo
