# Unpacking a tuple place copies each element into its target: the target
# owns its own `String`, and the tuple keeps the one it had. `_` reads
# nothing, so an element that cannot be copied implicitly may sit under it.
@fieldwise_init
struct Holder:
    var pair: Tuple[Int, String]


@fieldwise_init
struct Box[T: ImplicitlyCopyable & Writable & Deinitable]:
    var v: Self.T

    def show(self, t: Tuple[Self.T, Int]):
        var a, n = t
        print(a, n, self.v)


def take(t: Tuple[String, Int]):
    var s, n = t
    print(s, n)


def main():
    var t = (String("t"), 6)
    var s, n = t
    print(s, n)
    s += "x"
    print(s, t[0], n)
    take(t)
    take((String("u"), 7))

    var a = String("a")
    var k = 0
    print(a, k)
    a, k = t
    print(a, k)

    var pair: Tuple[Int, String] = (3, "seven")
    var first, second = pair
    var left: Int = pair[0]
    print(first, second, left)

    var h = Holder((1, String("q")))
    var i, q = h.pair
    print(i, q, h.pair[1])

    var mixed = (String("m"), 8, [1, 2])
    var m, _, _ = mixed
    var _, e, _ = mixed
    print(m, e, len(mixed[2]))

    var b = Box[String](String("b"))
    b.show(t)
    b.show(t)
