# A per-instantiation method clone inherits its checked template's facts when
# its body holds a `with` statement (`docs/notes/instantiation-from-template.md`,
# class MethodBody, feature `WITH_STATEMENTS`). The checker desugars the
# statement into ordinary statements whose shape the manager struct's
# declarations decide; the template keeps that form, and each instance builds
# its own desugar again from its syntax, naming the same synthesized
# occurrences. A plain `__exit__`, an error `__exit__` that swallows or
# re-raises, a consuming `__enter__` with and without `as`, a manager with no
# `__exit__`, a two-item `with`, and one inside a loop all derive for an `Int`
# and a `String` instance.
struct Guard(Movable):
    var tag: Int

    def __init__(out self, tag: Int):
        self.tag = tag

    def __enter__(self):
        print("enter", self.tag)

    def __exit__(self):
        print("exit", self.tag)


struct Tagged(Movable):
    var tag: Int

    def __init__(out self, tag: Int):
        self.tag = tag

    def __enter__(self) -> Int:
        return self.tag

    def __exit__(self):
        print("untag", self.tag)


struct Token(Movable):
    var tag: Int

    def __init__(out self, tag: Int):
        self.tag = tag

    def __enter__(var self) -> Self:
        print("take", self.tag)
        return self^

    def size(self) -> Int:
        return self.tag * 10

    def __deinit__(deinit self):
        print("drop", self.tag)


struct Held(Movable):
    var tag: Int

    def __init__(out self, tag: Int):
        self.tag = tag

    def __enter__(self):
        print("hold", self.tag)

    def __deinit__(deinit self):
        print("release", self.tag)


struct Swallow(Movable):
    var tag: Int

    def __init__(out self, tag: Int):
        self.tag = tag

    def __enter__(self):
        print("enter", self.tag)

    def __exit__(self):
        print("clean exit", self.tag)

    def __exit__(self, err: Error) -> Bool:
        print("swallowed", self.tag)
        return True


struct Rethrow(Movable):
    var tag: Int

    def __init__(out self, tag: Int):
        self.tag = tag

    def __enter__(self):
        print("enter", self.tag)

    def __exit__(self):
        print("clean exit", self.tag)

    def __exit__(self, err: Error) -> Bool:
        print("passing on", self.tag)
        return False


struct Bag[T: ImplicitlyCopyable & Deinitable](Movable):
    var items: List[Self.T]

    def __init__(out self, var items: List[Self.T]):
        self.items = items^

    def counted(self) -> Int:
        with Guard(1):
            return len(self.items)

    def tagged(self) -> Int:
        with Tagged(len(self.items)) as n:
            return n + 1

    def taken(self) -> Int:
        var total = len(self.items)
        with Token(2) as t:
            total += t.size()
        with Token(3):
            total += 1
        return total

    def held(self) -> Int:
        with Held(4):
            return len(self.items)

    def guarded(self) raises -> Int:
        with Swallow(5):
            if len(self.items) > 5:
                raise Error("too many")
        return len(self.items)

    def failing(self) raises -> Int:
        with Swallow(8):
            raise Error("boom")
        return len(self.items)

    def rethrown(self) raises -> Int:
        with Rethrow(9):
            if len(self.items) > 0:
                raise Error("again")
        return 0

    def looped(self) -> Int:
        var n = 0
        for _ in self.items:
            with Guard(10):
                n += 1
        return n

    def both(mut self, value: Self.T) -> Int:
        with Guard(6), Tagged(7) as n:
            self.items.append(value)
            return n + len(self.items)


def main() raises:
    var ints: List[Int] = [1, 2, 3]
    var strings: List[String] = [String("x")]
    var a = Bag[Int](ints^)
    var b = Bag[String](strings^)
    print(a.counted(), b.counted())
    print(a.tagged(), b.tagged())
    print(a.taken(), b.taken())
    print(a.held(), b.held())
    print(a.guarded(), b.guarded())
    print(a.failing(), b.failing())
    try:
        _ = a.rethrown()
    except e:
        print("caught", e)
    try:
        _ = b.rethrown()
    except e:
        print("caught", e)
    print(a.looped(), b.looped())
    print(a.both(4), b.both(String("y")))
