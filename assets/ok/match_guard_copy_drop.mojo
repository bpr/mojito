# A `var` case binding over a named subject copies before its guard runs
# and is destroyed after its last use, which may be before the guard call; a
# false guard advances to the next case; a temporary subject's last-use
# binding moves instead of copying. Output and destructor order match the
# pin.
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
    var t = Tracked(1)
    print("match")
    match t:
        case var x if guard(x.v):
            print("wrong")
        case var y:
            print("body", y.v)
            y.v = 5
    print("after", t.v)
    match Tracked(7):
        case var x if guard(x.v):
            print("wrong")
        case _:
            print("wildcard")
    print("end")
