# expect: cannot copy
# A `var` binding copies the subject with the implicit-copy rule, so a
# subject that is not `ImplicitlyCopyable` cannot bind by `var`.
struct Big(Movable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v


def main():
    var b = Big(1)
    match b:
        case var x:
            print(x.v)
