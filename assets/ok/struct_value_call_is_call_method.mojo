# A call on a struct value is its `__call__` method call, as upstream's
# `CallNode` resolves a callee that is no function: by name (`a(1)`, `self(x)`,
# a keyword argument), through a trait bound declaring `__call__`, through a
# field (`h.f(1)`), on an element (`objs[0](1)`, `g[1, 1](2)`), on an
# expression (`Add(7)(1)`), and with a bracket binding `__call__`'s own
# parameters when the struct declares no `__getitem__` (`s[1](3)`). A
# `mut self` `__call__` writes its receiver.
trait Caller:
    def __call__(self, x: Int) -> Int:
        ...


@fieldwise_init
struct Add(Caller, Copyable):
    var n: Int

    def __call__(self, x: Int) -> Int:
        return self.n + x

    def twice(self, x: Int) -> Int:
        return self(self(x))


@fieldwise_init
struct Acc(Copyable):
    var total: Int

    def __call__(mut self, x: Int) -> Int:
        self.total += x
        return self.total


@fieldwise_init
struct Holder(Copyable):
    var f: Add


@fieldwise_init
struct Grid(Copyable):
    var cell: Add

    def __getitem__(self, row: Int, column: Int) -> Add:
        return self.cell.copy()


@fieldwise_init
struct Scale(Copyable):
    var k: Int

    def __call__[n: Int](self, x: Int) -> Int:
        return self.k * x + n


def apply[F: Caller](f: F, x: Int) -> Int:
    return f(x)


def main():
    var a = Add(3)
    print(a(1), a(x=2), a.twice(1))
    print(apply(Add(3), 4))
    var h = Holder(Add(5))
    print(h.f(1))
    var objs = List[Add]()
    objs.append(Add(2))
    print(objs[0](1))
    var g = Grid(Add(10))
    print(g[1, 1](2))
    print(Add(7)(1))
    var s = Scale(2)
    print(s[1](3))
    var acc = Acc(0)
    _ = acc(2)
    print(acc(3), acc.total)
