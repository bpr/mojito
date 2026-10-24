# A read of a local beside a live `ref` binding to it, made directly, as a
# call argument, or through a nested function's `imm` capture: the binding
# names an origin and borrows nothing exclusively, so each read runs.


@no_inline
def reads(x: Int) -> Int:
    return x + 1


def plain(k: Int) -> Int:
    var kk = k
    ref r = kk
    var a = kk + 1
    return a + r


def through_call(k: Int) -> Int:
    var kk = k
    ref r = kk
    var a = reads(kk)
    return a + r


def through_nested(k: Int) -> Int:
    var kk = k

    @no_inline
    def cap(x: Int) {imm kk} -> Int:
        return kk + x

    ref r = kk
    var a = cap(1)
    return a + r


def main():
    print(plain(1), through_call(2), through_nested(2))
