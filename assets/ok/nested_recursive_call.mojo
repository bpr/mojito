def test(a: Int) -> Int:
    def _help(b: Int) -> Int:
        if b == 0:
            return 0
        return b + _help(b - 1)

    return _help(a)


def main():
    var a = test(10)
    print(a)
