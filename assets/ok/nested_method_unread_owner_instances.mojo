# A `def` nested in a parametric struct's method is a generator over the
# struct's parameters beside its enclosing method's and its own, as
# upstream's `RemoveUnusedParams` treats the lifted closure, so it keys its
# instances only on those it reads, and the method reads through it only
# the ones it kept: `Counter[1]` and `Counter[2]` share one `get` and one
# `bump` when neither reads `k`, one `st` and two `bump[m]` when the nested
# body reads only its own `m`, and one `get` and one `bump` when the loop a
# `comptime for` unrolls declares `bump`. A nested body reading `Self.k`
# keeps one instance per `k`, as does the method calling it; a method
# reading `Self.k` keeps one per `k` while its nested body keeps one; and a
# nested body taken as a value keeps one as well.
struct Counter[k: Int]:
    var v: Int

    @no_inline
    def __init__(out self, v: Int):
        self.v = v

    @no_inline
    def get(self) -> Int:
        @no_inline
        def bump(y: Int) -> Int:
            return y + 1

        return bump(self.v)

    @staticmethod
    @no_inline
    def st(x: Int) -> Int:
        @no_inline
        def bump[m: Int](y: Int) -> Int:
            return y + m

        return bump[1](x) + bump[2](x)

    @no_inline
    def loop_get(self) -> Int:
        var total = 0

        comptime for i in range(2):

            @no_inline
            def bump(y: Int) -> Int:
                return y + 1

            total += bump(self.v)
        return total

    @no_inline
    def nested_reads(self) -> Int:
        @no_inline
        def bump(y: Int) -> Int:
            return y + Self.k

        return bump(self.v)

    @no_inline
    def method_reads(self) -> Int:
        @no_inline
        def bump(y: Int) -> Int:
            return y + 1

        return bump(self.v) + Self.k

    @no_inline
    def valued(self) -> Int:
        @no_inline
        def bump(y: Int) -> Int:
            return y + 2

        var f = bump
        return f(self.v)


def main():
    print(Counter[1](1).get(), Counter[2](2).get())
    print(Counter[1].st(3), Counter[2].st(4))
    print(Counter[1](1).loop_get(), Counter[2](2).loop_get())
    print(Counter[1](1).nested_reads(), Counter[2](2).nested_reads())
    print(Counter[1](1).method_reads(), Counter[2](2).method_reads())
    print(Counter[1](1).valued(), Counter[2](2).valued())
