# A bare `ref` parameter is in `AddressSpace.GENERIC`, so it does not take
# a place reached through a pointer in another space, as at the pin.
# expect: operand address space '1' doesn't match expected address space '0'
from std.memory import AddressSpace


def g(ref x: Int) -> Int:
    return x


def main():
    var y = 3
    var p = Pointer(to=y).unsafe_address_space_cast[AddressSpace.GLOBAL]()
    print(g(p[]))
