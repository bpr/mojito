# An origin binder is a value of its `Origin` type, which does not convert
# to `Int`.
# expect: expected Int, found Origin[false]


def f[o: ImmOrigin](ref[o] x: Int):
    comptime b: Int = o
    print(b)


def main():
    var x = 1
    f(x)
