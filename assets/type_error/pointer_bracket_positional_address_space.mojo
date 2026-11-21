# `address_space` follows `Pointer`'s bare `*`, so it binds by keyword only.
# expect: unexpected parameter
from std.memory import AddressSpace


def main():
    var x = 5
    var p = Pointer(to=x)
    var r: Pointer[Int, origin_of(x), AddressSpace.GENERIC] = p
    print(r[])
