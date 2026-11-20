# Upstream's `AddressSpace` (`std.memory.address_space`), exported from
# `std.memory` and the prelude: its named cases print by name and any other
# space as `AddressSpace(N)`, `Int` reads its index, and `==`/`!=` compare
# indices. A parameter defaulted to `.GENERIC` resolves against its type.
# Upstream prints `AddressSpace.GENERIC`, `0`, `True`, `True`,
# `AddressSpace.GLOBAL`, `AddressSpace.SHARED`, `AddressSpace.LOCAL`,
# `AddressSpace(8)`, `AddressSpace(9)`, `3`, and `0`.
from std.memory import AddressSpace


def index_of[a: AddressSpace = .GENERIC]() -> Int:
    return Int(a)


def main():
    var a = AddressSpace.GENERIC
    print(a)
    print(Int(a))
    print(a == AddressSpace.GENERIC)
    print(a != AddressSpace.GLOBAL)
    print(AddressSpace.GLOBAL)
    print(AddressSpace.SHARED)
    print(AddressSpace.LOCAL)
    print(AddressSpace.BUFFER_RESOURCE)
    print(AddressSpace(9))
    print(index_of[AddressSpace.SHARED]())
    print(index_of())
