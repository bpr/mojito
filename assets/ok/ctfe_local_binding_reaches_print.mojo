# A local `comptime` binding that applies a callable runs at compile time,
# and a `print` it reaches prints while the program elaborates, once per
# instance: `mojito run` shows `side` before `main`'s output, as `mojo run`
# does, and a native executable never prints it.
def f(n: Int) -> Int:
    print("side")
    return n + 1


def g[k: Int]():
    comptime x = f(k)
    print(x)


def main():
    g[1]()
