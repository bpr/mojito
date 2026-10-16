# R1082: a read of a local while a `ref` binding to it is live is rejected
# with "access to 'kk' conflicts with live reference 'r'"; the pin prints
# `4 5 5`.


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
