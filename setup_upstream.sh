#!/usr/bin/env bash
# Run on the NEW machine: recreate ~/src/mojo/repos/modular and the pinned
# worktree ~/src/mojo/repos/modular-cfd79ca7 exactly as on the old machine.
set -euo pipefail

PIN=cfd79ca736884aefbb5636bc8eacc79888b233c6     # Mojo 1.2.0.dev2026101705 lockfiles
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

if [ ! -d ../modular-cfd79ca7 ]; then
    git worktree add --detach ../modular-cfd79ca7 "$PIN"
fi

echo
git -C ~/src/mojo/repos/modular log -1 --format='main:   %h %s'
git -C ~/src/mojo/repos/modular-cfd79ca7 log -1 --format='pinned: %h %s'
ls ~/src/mojo/repos/modular-cfd79ca7/Mojo/stdlib/std/simd.mojo
