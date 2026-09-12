# Omitted arguments whose default runs a converting constructor: the checker
# records the conversion at the default expression, and both backends run it
# at the call site the way an explicitly passed argument runs.
struct Meters(Copyable, Movable):
    var v: Int

    @implicit
    def __init__(out self, v: Int):
        self.v = v


@fieldwise_init
struct Row(Copyable, Movable):
    var base: Int

    def __getitem__(self, index: Int, fallback: Optional[Int] = None) -> Int:
        return self.base + index + fallback.or_else(100)


def filled(x: Optional[Int] = 5) -> Int:
    return x.or_else(0)


def empty(x: Optional[Int] = None) -> Int:
    return x.or_else(0)


def measured(m: Meters = 3) -> Int:
    return m.v


def padded(text: String = "ab", pad: StringSpan = " ") -> Int:
    return text.byte_length() + pad.byte_length()


def main():
    print(filled(), filled(7))
    print(empty(), empty(7))
    print(measured(), measured(9))
    print(padded(), padded(String("abc")))
    var row = Row(10)
    print(row[1], row[1, 2])
