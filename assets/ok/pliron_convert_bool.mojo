# `Bool` of a NaN is `__bool__`'s ordered `pop.cmp ne`, so a NaN made at
# run time is `False`; the pin's own compile-time fold of the same compare
# answers `True` (`conformance/probes/nan_truth_folded.mojo`).
def runtime_zero() -> Float64:
    var total = 0.0
    for i in range(2):
        total += Float64(i) - Float64(i)
    return total


def compute() -> Int:
    var flags = 0
    if Bool(3):
        flags = flags + 1
    if not Bool(0):
        flags = flags + 2
    var zero = runtime_zero()
    if Bool(zero / zero):
        flags = flags + 4
    if not Bool(0.0):
        flags = flags + 8
    var n = 7
    if Bool(n):
        flags = flags + 16
    if Int(True) == 1:
        flags = flags + 32
    if Float64(False) == 0.0:
        flags = flags + 64
    return flags

def main():
    print(compute())
