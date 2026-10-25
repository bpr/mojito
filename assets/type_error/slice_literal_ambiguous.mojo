# expect: ambiguous
# A slice literal binds every parameter type whose `__slice_literal__`
# constructor accepts its operands at one cost, so `c[1:3]` over both a
# `Slice` and a `ContiguousSlice` overload is ambiguous, as at the pin.
from std.builtin.builtin_slice import ContiguousSlice

struct C:
    def __init__(out self):
        pass

    def __getitem__(self, s: Slice) -> Int:
        return 1

    def __getitem__(self, s: ContiguousSlice) -> Int:
        return 2

def main():
    print(C()[1:3])
