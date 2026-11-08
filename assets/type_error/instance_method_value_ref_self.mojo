# A `ref self` method is parametric over its receiver's mutability and
# origin, which a read through its type binds neither of, so it is no
# runtime value.
# expect: cannot use parametric function as a runtime closure
struct S:
    var x: Int

    def __init__(out self, x: Int):
        self.x = x

    def r(ref self) -> ref[self.x] Int:
        return self.x


def main():
    var s = S(4)
    var g = S.r
    print(g(s))
