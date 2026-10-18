# PROBE (re-probe): does a `for` loop over `range` run its `else` clause
# when it completes?
#
# Upstream regression found at the 2026-10-17 re-pin. Mojo
# 1.2.0.dev2026092105 printed all three lines; Mojo 1.2.0.dev2026101705
# (46f49943) prints `list else` and `while else` only: a `for ... else`
# over `range` skips the `else` clause while the same loop over a `List`
# runs it. Mojito prints all three lines, which is what the manual and
# the previous pin say.
#
# Until upstream fixes the regression, `conformance/cases.tsv` carries
# `loop-completion` as an `output-diff` row instead of `run`. Flip it back
# when this probe prints `range else` again at the pin.
#
# Run:    mojo run for_else_over_range.mojo
#         cargo run -- run conformance/probes/for_else_over_range.mojo

def main():
    for _ in range(3):
        pass
    else:
        print("range else")
    var items = [1, 2]
    for _ in items:
        pass
    else:
        print("list else")
    var i = 0
    while i < 2:
        i += 1
    else:
        print("while else")
