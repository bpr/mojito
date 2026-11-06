# A generator read where a run-time value is needed is upstream's trivial
# register value of `!kgen.generator` type: it holds nothing, and it binds,
# copies, assigns, returns, fills a field, and passes to a generic argument.
comptime Add[a: Int, b: Int]: Int = a + b
comptime Mul[a: Int, b: Int]: Int = a * b


struct S:
    comptime k[i: Int]: Int = i + 1


struct H:
    var g: __generator_type[b: Int] Int

    def __init__(out self):
        self.g = Add[2, _]


def ret() -> __generator_type[b: Int] Int:
    return Add[1, _]


def cp[T: Copyable](x: T) -> T:
    return x.copy()


def id[T: AnyType](x: T):
    print("id")


def show[G: __generator_type[b: Int] Int]():
    var x = G
    id(x)
    print(G[2])


def main():
    var k = S.k
    _ = k
    var x = Add[1, _]
    x = Mul[2, _]
    var y = ret()
    _ = y
    var z = cp(x)
    _ = z
    var h = H()
    id(h.g)
    id(x)
    show[Add[1, _]]()
