# The canonical form of a parameter expression: integer `+`, `-`, and `*` are
# a sum of products with collected coefficients, and a left shift by a
# constant is a multiplication. Each equality below is accepted by the pinned
# Mojo with its parameters symbolic (`docs/notes/param-expr-attributes.md`).
struct Buf[n: Int](Copyable, Movable):
    var value: Int

    def __init__(out self, value: Int):
        self.value = value


def cancel[n: Int](var x: Buf[n - 1 + 1]) -> Buf[n]:
    return x^


def neutral[n: Int](var x: Buf[n * 1 + 0]) -> Buf[n]:
    return x^


def difference[n: Int](var x: Buf[2 * n - n]) -> Buf[n]:
    return x^


def square[n: Int](var x: Buf[(n + 1) * (n + 1)]) -> Buf[n * n + 2 * n + 1]:
    return x^


def commute[n: Int, m: Int](var x: Buf[n * m]) -> Buf[m * n]:
    return x^


def absorb[n: Int, m: Int](var x: Buf[n + m - m]) -> Buf[n]:
    return x^


def associate[n: Int](var x: Buf[n * n * n]) -> Buf[n * (n * n)]:
    return x^


def spread[n: Int, m: Int](var x: Buf[(n + m) * 2]) -> Buf[2 * m + 2 * n]:
    return x^


def shift[n: Int](var x: Buf[n << 1]) -> Buf[2 * n]:
    return x^


def conjugate[n: Int](var x: Buf[(n + 1) * (n - 1)]) -> Buf[n * n - 1]:
    return x^


def main():
    print(cancel[3](Buf[3](1)).value)
    print(neutral[3](Buf[3](2)).value)
    print(difference[3](Buf[3](3)).value)
    print(square[3](Buf[16](4)).value)
    print(commute[3, 4](Buf[12](5)).value)
    print(absorb[3, 4](Buf[3](6)).value)
    print(associate[3](Buf[27](7)).value)
    print(spread[3, 4](Buf[14](8)).value)
    print(shift[3](Buf[6](9)).value)
    print(conjugate[3](Buf[8](10)).value)
