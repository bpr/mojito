# A receiver clause's origin binder bound by the bracket must be the
# receiver's own origin, as at the pin.
# expect: operand origin 'origin_of(t)' doesn't match expected origin 'origin_of(s)'


struct S:
    var x: Int

    def __init__(out self):
        self.x = 1

    def first[o: Origin](ref[o] self) -> Int:
        return self.x


def main():
    var s = S()
    var t = S()
    print(t.first[origin_of(s)]())
