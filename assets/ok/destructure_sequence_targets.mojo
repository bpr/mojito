# Sequence destructuring `[t0, t1, ...] = value` over a `Sized` value:
# the value is evaluated once, upstream's `check_list_length` raises before
# any store on a wrong length, then each element is read through
# `value.__getitem__(i)` in order and stored into its target, whose own
# subexpressions evaluate after that read. `_` still reads; owned getter
# results move into `var` leaves; a `ref` leaf reads.
struct Box(Sized, Copyable):
    def __init__(out self):
        pass

    def __len__(self) -> Int:
        print("len")
        return 2

    def __getitem__(self, i: Int) -> Int:
        print("get", i)
        return i + 10


struct Tracked(Copyable):
    var n: Int

    def __init__(out self, n: Int):
        self.n = n

    def __init__(out self, *, copy: Self):
        self.n = copy.n
        print("copy", self.n)

    def __deinit__(deinit self):
        print("del", self.n)


struct Maker(Sized, Copyable):
    var k: Int

    def __init__(out self, k: Int):
        self.k = k

    def __len__(self) -> Int:
        print("mlen")
        return self.k

    def __getitem__(self, i: Int) -> Tracked:
        print("mget", i)
        return Tracked(i)


trait Seq(Sized):
    def __getitem__(self, i: Int) -> Int:
        ...


struct Two(Seq, Copyable):
    def __init__(out self):
        pass

    def __len__(self) -> Int:
        return 2

    def __getitem__(self, i: Int) -> Int:
        return i + 7


def pair_sum[T: Seq](x: T) raises -> Int:
    var [a, b] = x
    return a + b


def make() -> Box:
    print("make")
    return Box()


def idx(i: Int) -> Int:
    print("idx", i)
    return i


def main() raises:
    var xs: List[Int] = [0, 0, 0]
    [xs[idx(0)], xs[idx(1)]] = make()
    print(xs[0], xs[1])
    var bx = Box()
    var [a, _] = bx
    print(a)
    var mk = Maker(2)
    var [t, u] = mk
    print("bound")
    print(t.n)
    print(u.n)
    var [_, w] = mk
    print(w.n)
    var p = Tracked(100)
    var q = Tracked(200)
    [p, q] = mk
    print(p.n, q.n)
    try:
        var [e, f, g] = mk
        print(e.n, f.n, g.n)
    except err:
        print("caught", err)
    var pairs: List[Tuple[Int, Int]] = [(1, 2), (3, 4)]
    var [(c, d), (h, i)] = pairs
    print(c, d, h, i)
    var rows: List[List[Int]] = [[1, 2], [3]]
    var [[j, k], [l]] = rows
    print(j, k, l)
    var ys: List[Int] = [5, 6, 7]
    var s = 0
    [var m, ref r, s] = ys
    print(m, r, s)
    ys[0] = 50
    print(ys[0], m)
    print(pair_sum(Two()))
    var text = String("hello world")
    var [first, second] = text.split(" ")
    print(first, second)
    var [x, y] = [String("x"), String("y")]
    print(x, y)
