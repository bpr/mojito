# A `ref` parameter whose clause names an address space takes only a place
# in that space: a local lives in `AddressSpace.GENERIC`, as at the pin.
# expect: operand address space '0' doesn't match expected address space '1'
from std.memory import AddressSpace


def g(ref[_, AddressSpace(1)] x: Int) -> Int:
    return x


def main():
    var y = 3
    print(g(y))
