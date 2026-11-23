# A call through a trait bound that leaves out a defaulted parameter takes
# the requirement's default, as the requirement's declaration carries it,
# whatever the witness declares: a subscript (`x[1]`) as a method call
# does, and a default reading only some of the method's compile-time
# parameters whatever the others bind (`T` beside `n`), for a requirement
# and for a trait default a conformer overrides alike.


trait Idx:
    def __getitem__(self, i: Int, j: Int = 3) -> Int:
        ...


@fieldwise_init
struct A(Idx):
    var v: Int

    def __getitem__(self, i: Int, j: Int = 300) -> Int:
        return i + j


def subscript[T: Idx](x: T) -> Int:
    return x[1]


def called[T: Idx](x: T) -> Int:
    return x.__getitem__(1)


trait Shows:
    def required[T: Writable, n: Int](self, t: T, x: Int = n * 2) -> Int:
        ...

    def defaulted[T: Writable, n: Int](self, t: T, x: Int = n * 3) -> Int:
        print(t)
        return x


@fieldwise_init
struct Keeps(Shows):
    var v: Int

    def required[T: Writable, n: Int](self, t: T, x: Int = 1000) -> Int:
        return self.v + x


@fieldwise_init
struct Overrides(Shows):
    var v: Int

    def required[T: Writable, n: Int](self, t: T, x: Int = 1000) -> Int:
        return self.v + x

    def defaulted[T: Writable, n: Int](self, t: T, x: Int = 2000) -> Int:
        return self.v + x


def show[S: Shows](s: S) -> Int:
    return s.required[n=3]("r") + s.defaulted[n=5]("d") * 100


def main():
    var a = A(5)
    print(subscript(a))
    print(called(a))
    print(a[1])
    print(show(Keeps(1)))
    print(show(Overrides(1)))
