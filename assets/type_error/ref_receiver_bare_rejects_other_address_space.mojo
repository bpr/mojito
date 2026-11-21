# A bare `ref self` takes a receiver in `AddressSpace.GENERIC` only, so a
# place reached through a `GLOBAL` pointer is rejected, as at the pin.
# expect: operand address space '1' doesn't match expected address space '0'


struct S:
    var x: Int

    def __init__(out self):
        self.x = 1

    def get(ref self) -> Int:
        return self.x


def main():
    var s = S()
    var p = Pointer(to=s)
    var q = p.unsafe_address_space_cast[AddressSpace.GLOBAL]()
    print(q[].get())
