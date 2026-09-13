def _help(b: Int) -> Int:
    if b == 0:
        return 0
    return b + _help(b - 1)


def test(a: Int) -> Int:
    def call(b: Int) -> Int:
        return _help(b)

    return call(a)


def main():
    var a = test(10)
    print(a)
