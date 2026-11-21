# A `ref [o]` argument to a function applied at `origin_of(h)` must be `h`
# itself: its field `h.x` is a different origin, as at the pin.
# expect: origin-specialized function value
struct Holder:
    var x: Int

    def __init__(out self, x: Int):
        self.x = x


def read[o: Origin](ref [o] v: Int) -> Int:
    return v


def main():
    var h = Holder(1)
    print(read[origin_of(h)](h.x))
