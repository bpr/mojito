# A `var` binding in a failed case copies when a later case still reads the
# subject, which is evaluated once and survives; a failed `|` alternative
# runs its tests before any binding, so it copies nothing. The subject is
# read at the end, so no copy is its last use.
struct Tracked(ImplicitlyCopyable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v
        print("init", v)

    def __init__(out self, *, copy: Self):
        self.v = copy.v
        print("copy", self.v)

    def __deinit__(deinit self):
        print("del", self.v)


def guard(v: Int) -> Bool:
    print("guard", v)
    return False


def main():
    match Tracked(7):
        case var x if guard(x.v):
            print("wrong")
        case y:
            print("next", y.v)
    print("---")
    var t = Tracked(1)
    match (t, 0):
        case (var n, 1) | (var n, 0):
            print("alt", n.v)
    print("end", t.v)
