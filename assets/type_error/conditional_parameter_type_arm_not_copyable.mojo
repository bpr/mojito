# expect: cannot copy non-Copyable type 'Int if 0 < n else Plain'
# A conditional parameter type is copyable only when both its arms are:
# `Plain` is not, so the generic body cannot copy `x` whichever arm a call
# selects.
struct Plain:
    var v: Int

    def __init__(out self, v: Int):
        self.v = v


def g[n: Int](x: Int if n > 0 else Plain) -> Int:
    var y = x
    return 0


def main():
    _ = g[1](3)
