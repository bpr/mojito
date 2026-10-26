# A trait method may declare upstream's `/` and `*` markers: a requirement
# with keyword-only parameters is called by keyword through a bound, and a
# default with both markers forwards each parameter as its kind requires.


trait T:
    def f(self, *, k: Int) -> Int:
        ...

    def g(self, a: Int, /) -> Int:
        ...

    def h(self, a: Int, /, *, k: Int = 2) -> Int:
        return a * k


@fieldwise_init
struct S(ImplicitlyCopyable, T):
    var v: Int

    def f(self, *, k: Int) -> Int:
        return self.v + k

    def g(self, a: Int, /) -> Int:
        return self.v * a


def use[X: T](t: X) -> Int:
    return t.f(k=4) + t.g(10) + t.h(3) + t.h(3, k=5)


def main():
    print(use(S(1)))
    print(S(2).h(1), S(2).h(1, k=7))
