# A `ContiguousSlice` value does not convert to `Slice`: upstream declares
# no such conversion, only `@implicit StridedSlice(other: Slice)`. Both
# compilers reject the call; the pin says "cannot be converted from
# 'ContiguousSlice' to 'Slice'".
from std.builtin.builtin_slice import ContiguousSlice

def f(s: Slice) -> Int:
    return s.end.or_else(-1)

struct C:
    def __init__(out self):
        pass

    def __getitem__(self, s: ContiguousSlice) -> Int:
        return f(s)

def main():
    print(C()[1:4])
