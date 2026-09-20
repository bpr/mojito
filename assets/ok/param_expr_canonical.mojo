# Reordered, distributed, and collected parameter expressions are one type
# before any value is supplied: a value argument is a typed canonical
# expression (`docs/notes/param-expr-attributes.md`), and type equality over it is node identity.
struct Buf[n: Int](Copyable, Movable):
    var value: Int

    def __init__(out self, value: Int):
        self.value = value


def reorder[n: Int](var x: Buf[n + 1]) -> Buf[1 + n]:
    return x^


def distribute[n: Int](var x: Buf[(n + 1) * 4]) -> Buf[n * 4 + 4]:
    return x^


def collect[n: Int](var x: Buf[n + n]) -> Buf[2 * n]:
    return x^


def main():
    print(reorder[3](Buf[4](7)).value)
    print(distribute[3](Buf[16](8)).value)
    print(collect[3](Buf[6](9)).value)
