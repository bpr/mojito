# A per-instantiation method clone inherits its checked template's facts when
# the body declares a nested `def` with a whole-value literal default, a typed
# `raises`, or an `out` parameter (`docs/notes/instantiation-from-template.md`,
# class MethodBody, feature `NESTED_DEFS`): a string-literal default and a
# `None` default into `Optional`, each also passed explicitly; a nested `def`
# raising a closed error type; and an `out` parameter of a closed or a
# parameter-typed value, first or last.
@fieldwise_init
struct Low(ImplicitlyCopyable, Movable, Writable):
    def write_to(self, mut writer: Some[Writer]):
        writer.write("Low")

    def write_repr_to(self, mut writer: Some[Writer]):
        self.write_to(writer)


struct Shelf[T: Copyable & Deinitable](Movable):
    var items: List[Self.T]

    def __init__(out self):
        self.items = List[Self.T]()

    def joined(self, k: Int) -> String:
        def join(x: Int, sep: String = "-") -> String:
            return sep

        return join(k) + join(k, "+")

    def picked(self, k: Int) -> Int:
        def pick(x: Int, alt: Optional[Int] = None) -> Int:
            return x

        return pick(k) + pick(k, None)

    def typed(self, k: Int) -> Int:
        def check(x: Int) raises Low -> Int:
            if x < 0:
                raise Low()
            return x + 1

        try:
            return check(k) + check(-k)
        except e:
            return -k

    def made(self, k: Int) -> Int:
        def make(x: Int, out r: Int):
            r = x + 1

        return make(k)

    def leading(self, k: Int) -> Int:
        def make(out r: Int, x: Int):
            r = x + 2

        return make(k)

    def fresh(self, item: Self.T) -> Int:
        def build(x: Self.T, out r: List[Self.T]):
            r = List[Self.T]()
            r.append(x.copy())

        return len(build(item))


def main():
    var ints = Shelf[Int]()
    var words = Shelf[String]()
    print(ints.joined(3), words.joined(4))
    print(ints.picked(3), words.picked(4))
    print(ints.typed(3), words.typed(0))
    print(ints.made(3), words.made(4))
    print(ints.leading(3), words.leading(4))
    print(ints.fresh(1), words.fresh("a"))
