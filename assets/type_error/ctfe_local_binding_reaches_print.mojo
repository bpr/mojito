# expect: is not safe for VM-backed compile-time execution
# A local `comptime` binding that applies a callable runs at compile time,
# so a `print` it reaches is rejected, as the request path rejects any
# compile-time `print`; the pin prints at compile time (docs/non-goals.md).
def f(n: Int) -> Int:
    print("side")
    return n + 1


def g[k: Int]():
    comptime x = f(k)
    print(x)


def main():
    g[1]()
