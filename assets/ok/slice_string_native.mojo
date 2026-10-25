# A `Slice` writes as upstream's `Slice(start, end, step)` with `None` for an
# omitted bound, through `print`, `String(...)`, and a library struct's
# `write_to` that delegates to it.
from std.builtin.builtin_slice import ContiguousSlice

def main():
    var s = Slice(1, 3)
    print(s)
    print(String(s))
    print(String(ContiguousSlice(None, 4, None)))
