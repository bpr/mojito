# A call of a struct's member passes the member's parameters as values, as
# upstream's call does, so it reads only the struct arguments the member
# kept: `fg[n](b: NBox[n])` returning `b.get()` has one instance for every
# `n`, since `get` reads no struct parameter, as on the pin. So do `twice`,
# which calls `self.get()`, a subscript (`fsub`), an operator (`fop`), a
# prefix operator whose `__invert__` ignores `n` (`finv`), a generic method
# (`fput`, keyed on its inferred `T`; `fk`, on its explicit `k`), a `for`
# loop over an iterator `__iter__` builds from `self.v` (`fit`), and `ft`,
# whose `TBox[T].again` returns `self.copy()`. So does `fbound`, whose
# inlined `mylen[T: MySized]` calls `__len__` through its bound, which the
# inlining folds to `NBox.__len__`, the witness `NBox`'s conformance records.
# `fneg`, whose `__neg__` reads `Self.n`, keeps one instance per value, as on
# the pin.
trait MySized:
    def __len__(self) -> Int:
        ...


struct IntIter(Iterator, Movable):
    comptime Element = Int
    var i: Int

    def __init__(out self, i: Int):
        self.i = i

    @no_inline
    def __next__(mut self) raises StopIteration -> Int:
        if self.i <= 0:
            raise StopIteration()
        self.i -= 1
        return self.i


struct NBox[n: Int](Copyable, Movable, MySized):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    @no_inline
    def get(self) -> Int:
        return self.v

    @no_inline
    def twice(self) -> Int:
        return self.get() + self.get()

    @no_inline
    def __getitem__(self, i: Int) -> Int:
        return self.v + i

    @no_inline
    def __add__(self, o: Self) -> Int:
        return self.v + o.v

    @no_inline
    def __neg__(self) -> Int:
        return Self.n - self.v

    @no_inline
    def __invert__(self) -> Int:
        return self.v + 1

    @no_inline
    def put[T: Writable](self, x: T) -> Int:
        print(x)
        return self.v

    @no_inline
    def size[k: Int](self) -> Int:
        return self.v + k

    @no_inline
    def __iter__(self) -> IntIter:
        return IntIter(self.v)

    @no_inline
    def __len__(self) -> Int:
        return self.v


struct TBox[T: AnyType](Copyable, Movable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    @no_inline
    def again(self) -> Self:
        return self.copy()


@no_inline
def fg[n: Int](b: NBox[n]) -> Int:
    return b.get()


@no_inline
def fsub[n: Int](b: NBox[n]) -> Int:
    return b[3]


@no_inline
def fop[n: Int](b: NBox[n]) -> Int:
    return b + b


@no_inline
def fneg[n: Int](b: NBox[n]) -> Int:
    return -b


@no_inline
def finv[n: Int](b: NBox[n]) -> Int:
    return ~b


@no_inline
def fput[n: Int, T: Writable](b: NBox[n], x: T) -> Int:
    return b.put(x)


@no_inline
def fk[n: Int, k: Int](b: NBox[n]) -> Int:
    return b.size[k]()


@no_inline
def fit[n: Int](b: NBox[n]) -> Int:
    var s = 0
    for x in b:
        s += x
    return s


@always_inline("nodebug")
def mylen[T: MySized](value: T) -> Int:
    return value.__len__()


@no_inline
def fbound[n: Int](b: NBox[n]) -> Int:
    return mylen(b)


@no_inline
def ft[T: AnyType](b: TBox[T]) -> Int:
    return b.again().v


def main():
    print(fg(NBox[1](3)), fg(NBox[2](4)))
    print(NBox[1](1).twice(), NBox[2](2).twice())
    print(fsub(NBox[1](1)), fsub(NBox[2](2)))
    print(fop(NBox[1](1)), fop(NBox[2](2)))
    print(fneg(NBox[1](3)), fneg(NBox[2](4)))
    print(finv(NBox[1](3)), finv(NBox[2](4)))
    print(fput(NBox[1](1), 7), fput(NBox[2](2), 7))
    print(fk[1, 3](NBox[1](1)), fk[2, 3](NBox[2](2)))
    print(fit(NBox[1](3)), fit(NBox[2](4)))
    print(ft(TBox[Int](5)), ft(TBox[String](6)))
    print(fbound(NBox[1](3)), fbound(NBox[2](4)))
