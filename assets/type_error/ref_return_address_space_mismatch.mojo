# A reference-returning body returns a place in the address space its
# result clause names, as at the pin.
# expect: cannot return reference with incompatible address space: '0' vs '1'
from std.memory import AddressSpace


def g(ref x: Int) -> ref[x, AddressSpace(1)] Int:
    return x


def main():
    var y = 3
    print(g(y))
