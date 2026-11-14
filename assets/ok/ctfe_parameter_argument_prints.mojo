# A function applied in a parameter argument runs at compile time, once per
# instance, and a `print` it reaches prints while the program elaborates:
# `noisy 1` and `noisy 2` are the compiler's output, never the executable's.
def noisy(n: Int) -> Int:
    print("noisy", n)
    return n + 1


def g[m: Int]():
    print("g", m)


def f[n: Int]():
    g[noisy(n)]()


def main():
    f[1]()
    f[2]()
    f[1]()
