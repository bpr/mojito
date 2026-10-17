# A conditional expression's result is an owned temporary, so a place arm is
# copied into it, which a value that is not `ImplicitlyCopyable` rejects.
# expect: value of type 'M' cannot be implicitly copied, it does not conform to 'ImplicitlyCopyable' (conditional-expression arm)
struct M(Copyable, Movable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v


def pick(ref r: M, c: Bool):
    var t = r if c else M(3)
    print(t.v)


def main():
    var m = M(1)
    pick(m, True)
