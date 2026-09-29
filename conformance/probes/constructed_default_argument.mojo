# PROBE: a call leaving out a parameter whose default is a construction.
#
# **Differs natively.** The pin and Mojito's VM print `a`: the VM runs the
# default `String("a")` as a lowered default function. The native backend
# stops with "pliron backend: in `main`: unsupported evaluated default
# argument of `f` is not yet lowered natively". Filed in `docs/roadmap.md`
# §2. When `run --backend pliron` prints `a`, promote this file to
# `assets/ok/` with its manifest rows.
#
# Observed 2026-09-28 against `Mojo 1.2.0.dev2026092105 (e9569894)`.
#
# Run:    mojo run constructed_default_argument.mojo


def f(s: String = String("a")) -> String:
    return s


def main():
    print(f())
