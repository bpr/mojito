# expect: violated constraint
# A method's `where` clause over an origin's `mut=` binder reads the
# mutability the call binds it to: a pointer to a read-only argument binds
# `False`, which violates `where mut`, as at the pin.
struct S:
    var v: Int

    def __init__(out self):
        self.v = 3

    def m[
        mut: Bool, //, o: Origin[mut=mut]
    ](self, x: Pointer[Int, o]) -> Int where mut:
        return self.v


def read_only(s: S, x: Int) -> Int:
    return s.m(Pointer(to=x))


def main():
    print(read_only(S(), 1))
