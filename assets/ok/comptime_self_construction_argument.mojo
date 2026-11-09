# A `comptime` binding whose only construction is `Self()`, passed through
# a type alias, is a compile-time application over the struct's binders,
# readable as a parameter argument.
struct Holder(Copyable, ImplicitlyCopyable):
    var k: Int

    def __init__(out self, q: Plain):
        self.k = 5


comptime H = Holder


def show[h: Holder]():
    print(h.k)


struct Plain(Copyable, Defaultable, ImplicitlyCopyable):
    def __init__(out self):
        pass

    def get(self):
        comptime w = H(Self())
        show[w]()


struct Sized[n: Int](Copyable, ImplicitlyCopyable):
    var k: Int

    def __init__(out self, q: Q[Self.n]):
        self.k = Self.n


def show_sized[m: Int, //, s: Sized[m]]():
    print(s.k)


struct Q[n: Int](Copyable, Defaultable, ImplicitlyCopyable):
    def __init__(out self):
        pass

    def get(self):
        comptime S = Sized[Self.n]
        comptime w = S(Self())
        show_sized[w]()


def main():
    Plain().get()
    Q[7]().get()
