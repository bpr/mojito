# A drop, copy, or move reads only the struct arguments the lifecycle
# member it runs kept, as upstream's destructor and copy/move insertion
# calls the member with the struct's kept parameters: `fd`, `fw`, `ft`,
# `fi`, `fp`, and `fm` have one instance for every argument, while `fk`,
# whose destructor reads `Self.n`, keeps one per `n`, as on the pin.


struct DBox[n: Int](Movable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    @no_inline
    def __deinit__(deinit self):
        print("del", self.v)


struct Wrap[n: Int](Movable):
    var b: DBox[Self.n]

    def __init__(out self, var b: DBox[Self.n]):
        self.b = b^


struct TBox[T: AnyType](Movable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    @no_inline
    def __deinit__(deinit self):
        print("tdel", self.v)


struct KBox[n: Int](Movable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    @no_inline
    def __deinit__(deinit self):
        print("kdel", self.v, Self.n)


struct IBox[n: Int](ImplicitlyCopyable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    @no_inline
    def __init__(out self, *, copy: Self):
        print("copy", copy.v)
        self.v = copy.v


struct Pair[n: Int](ImplicitlyCopyable):
    var a: IBox[Self.n]

    def __init__(out self, v: Int):
        self.a = IBox[Self.n](v)


struct MBox[n: Int](Movable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    @no_inline
    def __init__(out self, *, deinit move: Self):
        print("move", move.v)
        self.v = move.v


@no_inline
def fd[n: Int](var b: DBox[n]):
    print("fd", b.v)


@no_inline
def fw[n: Int](var w: Wrap[n]):
    print("fw", w.b.v)


@no_inline
def ft[T: AnyType](var b: TBox[T]):
    print("ft", b.v)


@no_inline
def fk[n: Int](var b: KBox[n]):
    print("fk", b.v)


@no_inline
def fi[n: Int](b: IBox[n]) -> Int:
    var c = b
    return c.v


@no_inline
def fp[n: Int](p: Pair[n]) -> Int:
    var c = p
    return c.a.v


@no_inline
def fm[n: Int](var b: MBox[n]) -> Int:
    var c = b^
    var d = c^
    return d.v


def main():
    fd(DBox[1](1))
    fd(DBox[2](2))
    fw(Wrap[1](DBox[1](3)))
    fw(Wrap[2](DBox[2](4)))
    ft(TBox[Int](5))
    ft(TBox[String](6))
    fk(KBox[1](7))
    fk(KBox[2](8))
    print(fi(IBox[1](9)), fi(IBox[2](10)))
    print(fp(Pair[1](11)), fp(Pair[2](12)))
    print(fm(MBox[1](13)), fm(MBox[2](14)))
