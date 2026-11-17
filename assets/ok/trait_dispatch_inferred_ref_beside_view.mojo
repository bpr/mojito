# A trait-dispatched call binding a mutable place to a `ref` slot whose
# mutability the requirement infers lends the place as a shared read: the
# slot's declared contract does not write, so the call runs beside a live
# `Span` of the same list, as at the pin, which prints `13 3` and `4 3`.
trait Sizer:
    def size(self, ref x: List[Int]) -> Int:
        ...


@fieldwise_init
struct S(Copyable, Sizer):
    var k: Int

    def size(self, ref x: List[Int]) -> Int:
        return len(x) + self.k


def run[T: Sizer](t: T):
    var a: List[Int] = [1, 2, 3]
    var s = Span(a)
    var n = t.size(a)
    print(n, len(s))


def main():
    run(S(10))
    var a: List[Int] = [1, 2, 3]
    var s = Span(a)
    print(S(1).size(a), len(s))
