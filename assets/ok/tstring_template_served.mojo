# A t-string in a template-served generic body, in a generic struct's
# method, and over a Copyable place that is not ImplicitlyCopyable.


def show[T: Writable & Copyable](x: T):
    var t = t"v={x};"
    print(t)
    print(String(t).byte_length())


struct Box[T: Writable & Copyable & Deinitable]:
    var v: Self.T

    def __init__(out self, v: Self.T):
        self.v = v.copy()

    def show(self):
        print(t"box={self.v}")


def imp[T: Writable & ImplicitlyCopyable](x: T):
    print(t"imp={x}")


def main():
    show(3)
    show("hi")
    Box(7).show()
    Box(String("s")).show()
    imp(4)
    imp(String("z"))
    var l: List[Int] = [1, 2]
    var t = t"l={l}"
    print(t)
    print(len(l))
    print(t"call={String("x")} tmp={[4, 5]}")
