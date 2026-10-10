# Defaults are inherited through refinement, once however many paths reach
# them, and a default dispatches through the bound to the conformer's own
# methods, whether spelled or themselves inherited.
trait A:
    def a(self) -> Int:
        return 1

    def twice(self) -> Int:
        return self.a() * 2

trait B(A):
    def b(self) -> Int:
        return self.a() + 10

trait C(A):
    def c(self) -> Int:
        return self.a() + 20

@fieldwise_init
struct S(B, C):
    var x: Int

@fieldwise_init
struct V(B):
    var x: Int

    def a(self) -> Int:
        return self.x

def use_a[X: A](x: X) -> Int:
    return x.twice()

def main():
    print(S(0).a(), S(0).b(), S(0).c(), S(0).twice())
    print(V(7).b(), use_a(V(7)), use_a(S(0)))
