# A `ref[...]` specifier names its address space at most once, as at the
# pin.
# expect: address space must be specified once
from std.memory import AddressSpace


def g(ref[_, AddressSpace.GENERIC, AddressSpace.GENERIC] x: Int) -> Int:
    return x


def main():
    var y = 3
    print(g(y))
