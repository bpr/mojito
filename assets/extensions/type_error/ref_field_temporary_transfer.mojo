# expect: expression does not designate a value with an origin
# A `ref` field read off a temporary holder still reaches its referent, not
# the temporary's storage, so `^` cannot move out of it.
struct Holder[o: MutOrigin]:
    var r: ref[Self.o] String

    def __init__(out self, ref[Self.o] s: String):
        self.r = s


def take(var s: String):
    print(s)


def main():
    var s = String("hi")
    take(Holder(s).r^)
