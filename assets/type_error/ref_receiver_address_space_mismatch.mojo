# A `ref[...] self` clause naming an address space takes only a receiver
# in that space: a local lives in `AddressSpace.GENERIC`, as at the pin.
# expect: operand address space '0' doesn't match expected address space '1'


struct S:
    var x: Int

    def __init__(out self):
        self.x = 1

    def get(ref[_, AddressSpace.GLOBAL] self) -> Int:
        return self.x


def main():
    var s = S()
    print(s.get())
