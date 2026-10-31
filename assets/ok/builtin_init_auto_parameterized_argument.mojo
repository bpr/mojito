# A builtin `@implicit` initializer whose argument's type names one of its
# own value binders folds at compile time: its body reads the binder
# through the argument (`w.v`, or `Self.v` inside `w.get()`), whether the
# binder is explicit (`w: W[k]`) or auto-parameterized (`w: W`).
struct W[v: Int](ImplicitlyCopyable, TrivialRegisterPassable):
    @always_inline("builtin")
    def __init__(out self):
        pass

    @always_inline("builtin")
    def get(self) -> Int:
        return Self.v


struct Len(ImplicitlyCopyable, TrivialRegisterPassable):
    var n: Int

    @implicit
    @always_inline("builtin")
    def __init__(out self, w: W):
        self.n = w.get()

    @implicit
    @always_inline("builtin")
    def __init__(out self, n: Int):
        self.n = n


struct Field(ImplicitlyCopyable, TrivialRegisterPassable):
    var n: Int

    @implicit
    @always_inline("builtin")
    def __init__(out self, w: W):
        self.n = w.v


struct Explicit(ImplicitlyCopyable, TrivialRegisterPassable):
    var n: Int

    @implicit
    @always_inline("builtin")
    def __init__[k: Int, //](out self, w: W[k]):
        self.n = w.v


struct Tag[l: Len]:
    def __init__(out self):
        pass

    @staticmethod
    def show():
        print(Self.l.n)


struct FieldTag[f: Field]:
    @staticmethod
    def show():
        print(Self.f.n)


struct ExplicitTag[e: Explicit]:
    @staticmethod
    def show():
        print(Self.e.n)


def main():
    var a: Tag[Len(W[3]())] = Tag[Len(3)]()
    a.show()
    Tag[W[4]()].show()
    FieldTag[Field(W[5]())].show()
    FieldTag[W[6]()].show()
    ExplicitTag[Explicit(W[7]())].show()
    ExplicitTag[W[8]()].show()
