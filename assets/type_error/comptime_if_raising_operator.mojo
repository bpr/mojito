# expect: cannot call raising function in 'comptime if' condition
# A compile-time condition the elaborator evaluates may not call a raising
# operator method.
@fieldwise_init
struct R(Copyable, ImplicitlyCopyable):
    var a: Int

    def __eq__(self, other: Int) raises -> Bool:
        return self.a == other


def f[n: Int]() raises:
    comptime q = R(n)
    comptime if q == 1:
        print("q")


def main() raises:
    f[1]()
