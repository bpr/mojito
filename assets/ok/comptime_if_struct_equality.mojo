# A compile-time comparison of a struct value runs the struct's own
# `__eq__`, in a `comptime if` condition and in a `comptime match` test: it
# is a value the elaborator evaluates, never a structural proposition.
@fieldwise_init
struct R(Copyable, ImplicitlyCopyable):
    var a: Int

    def __eq__(self, other: Int) -> Bool:
        return self.a == other


def f[n: Int]():
    comptime q = R(n)
    comptime if q == 1:
        print("q one")
    else:
        print("q other")
    comptime match q:
        case 1:
            print("one")
        case _:
            print("other")


def main():
    f[1]()
    f[2]()
