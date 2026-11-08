# A `deinit` parameter other than `self` is torn down by parts, as upstream's
# `deinit` convention disables the whole value's destructor: an
# `@explicit_destroy` value passed to one owes no explicit destruction, and
# each of its fields is destroyed at the callee's end.
struct N(Movable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __deinit__(deinit self):
        print("del", self.v)


@explicit_destroy("use eat")
struct W(Movable, Deinitable where False):
    var a: N
    var b: N

    def __init__(out self, var a: N, var b: N):
        self.a = a^
        self.b = b^

    def eat(deinit self, deinit other: W):
        print("eat")


def main():
    var w = W(N(1), N(2))
    var x = W(N(3), N(4))
    w^.eat(x^)
    print("end")
