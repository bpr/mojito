# A compile-time `//` by zero over an `Int` or `UInt` value, and a `//` or
# `%` by zero in a parameter position, fold to 0, as `__floordiv__` and
# `__mod__` mask a zero divisor.
comptime g = Int(7) // 0


struct W[k: Int]:
    @staticmethod
    def get() -> Int:
        return Self.k


def f[n: Int]() -> Int:
    return W[n % 0].get()


def q[n: Int]() -> Int:
    return W[n // 0].get()


def h[n: UInt]() -> UInt:
    comptime k = n // 0
    return k


def main():
    print(g, f[7](), q[7](), h[9]())
