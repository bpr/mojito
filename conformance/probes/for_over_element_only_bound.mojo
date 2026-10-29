# PROBE: a `for` loop over a value whose bound declares only `Element`.
#
# **Differs.** The pin rejects it, "'C' does not implement the '__iter__'
# method": a trait that declares no `__iter__` is not iterable. Mojito
# accepts it and iterates the bound's `Element`. Filed in
# `docs/roadmap.md` (R1615). `tests/checker_test.rs`'s
# `abstract_element_copyability_derives_from_member_bounds` exercises the
# accepted form until then.
#
# Observed 2026-10-28 against `Mojo 1.2.0.dev2026101705 (46f49943)`.
#
# Run:    mojo run for_over_element_only_bound.mojo


trait Items:
    comptime Element: ImplicitlyCopyable & Movable


def first[C: Items](xs: C) raises -> C.Element:
    for item in xs:
        return item
    raise "empty"


def main():
    print(1)
