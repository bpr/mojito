# A value declared before a `try` whose last use in the body comes after a
# raising call dies on that call's raise edge, before the handler runs, as
# the pin treats it as dead from the raise on. When the call returns, the
# value lives to its last use as usual.
#
# Expected output:
#   mdel 2
#   caught neg
#   d 3
#   mdel 3
#   done


struct M(Copyable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __init__(out self, *, copy: Self):
        self.v = copy.v

    def __deinit__(deinit self):
        print("mdel", self.v)


def may(x: Int) raises -> Int:
    if x < 0:
        raise Error("neg")
    return x


def run(x: Int):
    var m = M(x)
    try:
        _ = may(x - 3)
        var d = M(copy=m)
        print("d", d.v)
    except e:
        print("caught", e)


def main():
    run(2)
    run(3)
    print("done")
