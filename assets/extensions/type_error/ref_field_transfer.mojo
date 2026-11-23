# expect: expression does not designate a value with an origin
# A `ref` field does not own its referent: `^` through it reaches no tracked
# root, as a transfer out of a `ref` binding reaches none.
struct Holder[o: MutOrigin]:
    var r: ref[Self.o] String

    def __init__(out self, ref[Self.o] s: String):
        self.r = s


def take(var s: String):
    print(s)


def main():
    var s = String("hi")
    var h = Holder(s)
    take(h.r^)
