# An `out self` initializer whose aborting arm leaves a field unset still
# initializes every field on each path that completes: the `abort(...)` call
# returns `Never`, so that arm produces no value. The pin prints `3 3`.
from std.os import abort


struct S:
    var a: Int
    var b: Int

    def __init__(out self, x: Int):
        self.a = x
        if x < 0:
            abort("neg")
        else:
            self.b = x


def main():
    var s = S(3)
    print(s.a, s.b)
