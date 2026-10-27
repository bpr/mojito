# A walrus stores through any lvalue an assignment takes, and yields the
# stored value: a field, a list element, a nominal subscript through its
# setter (the target's indices evaluated before the value), a call with
# explicit parameters returning a mutable reference, and a nested walrus.
struct Box(Movable):
    var v: List[Int]

    def __init__(out self):
        self.v = [0, 0]

    def __getitem__(self, i: Int) -> Int:
        return self.v[i]

    def __setitem__(mut self, i: Int, value: Int):
        print("set", i, value)
        self.v[i] = value


struct W:
    var x: Int

    def __init__(out self):
        self.x = 1

    def at[k: Int](mut self) -> ref[self.x] Int:
        return self.x


@fieldwise_init
struct P(Copyable, Writable):
    var x: Int

    def bump(mut self) -> Int:
        return (self.x := self.x + 10)


def idx() -> Int:
    print("idx")
    return 1


def val() -> Int:
    print("val")
    return 7


def main():
    var xs: List[Int] = [1, 2, 3]
    print(xs[0] := 5)
    print(xs[0])

    var b = Box()
    print(b[idx()] := val())
    print(b.v[1])

    var a: List[Int] = [1, 2]
    var c: List[Int] = [3, 4]
    print(a[0] := (c[1] := 9))
    print(a[0], c[1])

    var w = W()
    w.at[0]() = 5
    w.at[0]() += 1
    print(w.x)
    print(w.at[0]() := 9)
    print(w.x)

    var p = P(1)
    print(p.bump(), p.x)

    var names: List[String] = ["a", "b"]
    print(names[0] := String("zz"))
    print(names[0], names[1])

    var fs: List[Float64] = [0.5]
    print(fs[0] := 1)
