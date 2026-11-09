# Each `Some[Trait]` parameter is its own anonymous `[T: Trait]` binder,
# so two of one trait deduce different types at one call.

trait Shower:
    def show(self, a: Some[Writable], b: Some[Writable]): ...

@fieldwise_init
struct S(Shower):
    var x: Int

    def show(self, a: Some[Writable], b: Some[Writable]):
        print(self.x, a, b)

def show_all[T: Shower](t: T):
    t.show(2.5, "z")

def three(a: Some[Writable], b: Some[Writable], c: Some[Writable]):
    print(a, b, c)

def same(a: Some[Writable], b: Some[Writable]):
    print(a, b)

def main():
    var s = S(7)
    s.show(String("a"), 1)
    s.show(b=True, a=3)
    show_all(s)
    three(1, "x", 2.0)
    three(b=1, c="x", a=2.0)
    same(1, 2)
    same("p", "q")
