# A value an erased clone builds keeps its struct's members: `make` reads no
# `n`, so its clone builds `B[T, _REMOVED_ARG]`, and the caller's display
# still finds `B[String, 3]`'s `write_to`, as upstream's value is its layout,
# which an unread parameter does not change. A compile-time value and a
# struct's `comptime` member of an instance over an unread parameter display
# the same way.
@fieldwise_init
struct B[T: ImplicitlyCopyable & Writable & Deinitable, n: Int](Copyable, Writable):
    var v: Self.T

    @staticmethod
    def make(v: Self.T) -> Self:
        return Self(v)

    def write_to(self, mut writer: Some[Writer]):
        writer.write("B(", self.v, ")")


@fieldwise_init
struct S[n: Int](ImplicitlyCopyable, Writable):
    var x: Int
    comptime Z = Self(7)

    def write_to(self, mut writer: Some[Writer]):
        writer.write(self.x)


def g[T: ImplicitlyCopyable & Writable & Deinitable, n: Int](x: T):
    print(B[T, n].make(x))


def h[T: ImplicitlyCopyable & Writable & Deinitable, n: Int](b: B[T, n]):
    print(type_of(b).make(b.v))


def main():
    g[String, 3]("a")
    h(B[String, 4]("c"))
    comptime z = S[2](5)
    print(z)
    print(S[2].Z)
