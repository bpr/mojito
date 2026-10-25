# A display or comprehension passed to a read parameter is a temporary the
# caller owns, as a call's result is: it lives in the caller's frame and
# dies right after the call that reads it. An `in` test's temporary
# container and a scalar conversion's temporary argument die the same way,
# right after the call that reads them.
struct M(Copyable, Movable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __init__(out self, *, copy: Self):
        self.v = copy.v + 10
        print("copy", copy.v)

    def __deinit__(deinit self):
        print("del", self.v)


struct Box(Boolable, Movable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __bool__(self) -> Bool:
        return self.v > 0

    def __deinit__(deinit self):
        print("del box", self.v)


struct Bag(Movable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __contains__(self, x: Int) -> Bool:
        return x == self.v

    def __deinit__(deinit self):
        print("del bag", self.v)


def count(xs: List[M]):
    print("count", len(xs))


def pair(t: Tuple[M, Int]):
    print("pair", t[0].v, t[1])


def size(d: Dict[Int, M]):
    print("size", len(d))


def take(var xs: List[M]):
    print("take", len(xs))


def make_bag(v: Int) -> Bag:
    return Bag(v)


def make_box(v: Int) -> Box:
    return Box(v)


def make_list() -> List[String]:
    return ["a", "b"]


def main():
    count([M(1), M(2)])
    print("after list")
    pair((M(3), 4))
    print("after tuple")
    size({5: M(5)})
    print("after dict")
    count([M(i) for i in range(6, 8)])
    print("after comprehension")
    take([M(8)])
    print("after take")
    _ = [M(9)]
    print("after discard")
    print("b" in make_list())
    print(2 in make_bag(2))
    print(3 not in make_bag(2))
    print("after contains")
    print(Bool(make_box(1)))
    print("after bool")
