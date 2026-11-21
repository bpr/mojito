# A `ref[o] self` clause over a `MutOrigin` binder takes only a mutable
# receiver: a read-only argument is rejected, as at the pin.
# expect: operand mutability 'False' doesn't match expected mutability 'True'


struct S:
    var x: Int

    def __init__(out self):
        self.x = 1

    def wr[o: MutOrigin](ref[o] self):
        self.x = 9


def h(s: S):
    s.wr()


def main():
    var s = S()
    h(s)
    print(s.x)
