# A receiver clause's address-space binder bound by the bracket is checked
# against the receiver's place, as a `ref` parameter's is.
# expect: operand address space '0' doesn't match expected address space '1'


struct S:
    var x: Int

    def __init__(out self):
        self.x = 1

    def space[a: AddressSpace](ref[_, a] self) -> Int:
        return Int(a._value) + self.x


def main():
    var s = S()
    print(s.space[AddressSpace.GLOBAL]())
