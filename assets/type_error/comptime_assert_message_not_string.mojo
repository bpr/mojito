# expect: cannot implicitly convert 'Int' to a `StringSpan`
# A `comptime assert` message must convert to a `StringSpan`, as at the pin.

def f[n: Int]():
    comptime assert n > 5, n
    print(n)


def main():
    f[7]()
