# A `for` loop calls a raising `__iter__` as an ordinary raising call, as
# upstream's `emitForStmt` does: its raise leaves through the call's raise
# edge, so the values live there are destroyed (`del 1` before `caught`) and
# the enclosing `try` catches it. Covered: a temporary, a named, and an owned
# (`b^`) source, a comprehension, and a generic iterable, each on its normal
# path too.
@fieldwise_init
struct Held(Movable):
    var v: Int

    def __deinit__(deinit self):
        print("del", self.v)


@fieldwise_init
struct BagIter[mut: Bool, //, origin: Origin[mut=mut]](Iterator):
    comptime Element = String
    var bag: Pointer[Bag, Self.origin]
    var i: Int

    def __next__(mut self) raises StopIteration -> String:
        if self.i >= len(self.bag[].items):
            raise StopIteration()
        self.i += 1
        return self.bag[].items[self.i - 1]


@fieldwise_init
struct OwnedIter(Iterator):
    comptime Element = String
    var items: List[String]

    def __next__(mut self) raises StopIteration -> String:
        if len(self.items) == 0:
            raise StopIteration()
        return self.items.pop(0)


struct Bag(Movable):
    var items: List[String]
    var fail: Bool

    def __init__(out self, fail: Bool):
        self.items = ["a", "b"]
        self.fail = fail

    def __iter__(ref self) raises -> BagIter[origin_of(self)]:
        if self.fail:
            raise Error("bag " + self.items[0])
        return BagIter(Pointer(to=self), 0)

    def __iter__(var self) raises -> OwnedIter:
        if self.fail:
            raise Error("owned bag")
        return OwnedIter(self.items.copy())


@fieldwise_init
struct Count(Iterator):
    comptime Element = Int
    var n: Int

    def __next__(mut self) raises StopIteration -> Int:
        if self.n == 0:
            raise StopIteration()
        self.n -= 1
        return self.n


@fieldwise_init
struct Gen[T: Copyable]:
    var n: Int

    def __iter__(ref self) raises -> Count:
        if self.n < 0:
            raise Error("negative")
        return Count(self.n)


def named(fail: Bool) raises:
    var b = Bag(fail)
    var k = Held(1)
    for x in b:
        print(x, k.v)
    print("after named", len(b.items))


def temporary(n: Int) raises:
    var k = Held(2)
    for x in Gen[String](n):
        print("temporary", x, k.v)


def owned(fail: Bool) raises:
    var b = Bag(fail)
    for x in b^:
        print("owned", x)


def comprehension(fail: Bool) raises:
    var b = Bag(fail)
    var xs = [x + "!" for x in b]
    print(len(xs), xs[0])


def main():
    try:
        var k = Held(0)
        for x in Gen[Int](-1):
            print(x)
    except e:
        print("caught", e)
    for fail in [False, True]:
        try:
            named(fail)
        except e:
            print("caught", e)
        try:
            owned(fail)
        except e:
            print("caught", e)
        try:
            comprehension(fail)
        except e:
            print("caught", e)
    for n in [2, -1]:
        try:
            temporary(n)
        except e:
            print("caught", e)
