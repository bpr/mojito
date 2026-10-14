# PROBE: a `Dict` entry's key read through `items()` in a generic `def`.
#
# The pin rejects `print(e.key)`: "an element of 'values' with type 'K' does
# not conform to trait 'Writable'", though the caller's `K` is `Writable`;
# `d.keys()` or a `var kk: K = e.key.copy()` binding is accepted. Mojito
# accepts `print(e.key)` and prints 1.
#
# Observed 2026-10-13 against `Mojo 1.2.0.dev2026092105 (e9569894)`.
#
# Run:    mojo run dict_entry_key_keeps_caller_bound.mojo
#         cargo run -- run conformance/probes/dict_entry_key_keeps_caller_bound.mojo
def f[K: Hashable & Equatable & Copyable & Deinitable & Writable](k: K) raises:
    var d = Dict[K, Int]()
    d[k.copy()] = 1
    for e in d.items():
        print(e.key)


def main() raises:
    f(1)
