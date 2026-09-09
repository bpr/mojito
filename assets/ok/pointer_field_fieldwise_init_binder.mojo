# A `@fieldwise_init` struct with a parametric-mutability origin binder
# (`m: Bool, //, o: Origin[mut=m]`) accepts `Pointer(to=x)` for a field of
# type `Pointer[T, Self.o]` from every place kind: a `var` local, a read
# parameter, a `ref` parameter, and a field reached through `self`. The
# binder takes its mutability from the place; only a binder spelled
# `mut=True` demands a mutable place.
@fieldwise_init
struct P[m: Bool, //, o: Origin[mut=m]]:
    var src: Pointer[List[Int], Self.o]
    var index: Int

    def get(self) -> Int:
        return self.src[][self.index]

def show(xs: List[Int]):
    var p = P(Pointer(to=xs), 0)
    print(p.get())

def show_ref(ref xs: List[Int]):
    var p = P(Pointer(to=xs), 1)
    print(p.get())

struct Holder:
    var items: List[Int]

    def __init__(out self):
        self.items = List[Int]()
        self.items.append(4)

    def head(self) -> Int:
        var p = P(Pointer(to=self.items), 0)
        return p.get()

def main():
    var xs = List[Int]()
    xs.append(7)
    xs.append(8)
    var p = P(Pointer(to=xs), 0)
    print(p.get())
    show(xs)
    show_ref(xs)
    print(Holder().head())
