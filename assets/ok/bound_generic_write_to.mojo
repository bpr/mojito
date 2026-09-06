# `write_to(writer)` on a receiver without a `write_to` body of its own — a
# `Writable`-bounded type parameter inside a generic struct's `write_to`, and
# the built-in Writable values (`Int`, `Float64`, `Bool`, `String`) — is the
# `writer.write(x)` shape: the checker records the operand swap and the
# argument formats through its own conformance (instance-clone aware for a
# struct). A user trait may bound a struct parameter regardless of the
# declaration order (`Show` below is declared after its first use).

struct Wrap[T: Writable & ImplicitlyCopyable & Deinitable](Writable):
    var value: Self.T

    def __init__(out self, value: Self.T):
        self.value = value

    def write_to[W: Writer](self, mut writer: W):
        writer.write("Wrap(")
        self.value.write_to(writer)
        writer.write(")")


struct Pair(Writable):
    var x: Int
    var s: String

    def __init__(out self, x: Int):
        self.x = x
        self.s = String("s")

    def write_to[W: Writer](self, mut writer: W):
        self.x.write_to(writer)
        writer.write("/")
        self.s.write_to(writer)
        writer.write("/")
        Float64(2.5).write_to(writer)
        writer.write("/")
        True.write_to(writer)


struct Boxed[T: Show & Copyable & Deinitable]:
    var value: Self.T

    def __init__(out self, var value: Self.T):
        self.value = value^

    def shown(self) -> Int:
        return self.value.show()


trait Show:
    def show(self) -> Int:
        ...


struct Num(Show, Copyable):
    var n: Int

    def __init__(out self, n: Int):
        self.n = n

    def show(self) -> Int:
        return self.n * 10


def main():
    print(Wrap(8))
    print(Pair(3))
    print(String(Wrap(2.5)))
    var out = String("")
    Int(42).write_to(out)
    Float64(1.5).write_to(out)
    print(out)
    print(Boxed(Num(4)).shown())
