# A `ref[ImmStaticOrigin] self` clause takes only a receiver carrying that
# exact origin: a tracked local does not convert, as at the pin.
# expect: operand origin 'origin_of(s)' doesn't match expected origin 'ImmStaticOrigin'


struct S:
    var x: Int

    def __init__(out self):
        self.x = 1

    def get(ref[ImmStaticOrigin] self) -> Int:
        return self.x


def main():
    var s = S()
    print(s.get())
