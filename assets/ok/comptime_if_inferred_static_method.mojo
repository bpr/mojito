# A method whose `comptime if` keys on its own type parameter specializes
# per inferred call, static or not: `S.show(3)` selects the `Int` arm
# without spelling `S.show[Int]`, including from another generic method's
# instantiations and on a parametric owner.

@fieldwise_init
struct S(Copyable):
    @staticmethod
    def show[T: Copyable](x: T):
        comptime if T == Int:
            print("int")
        else:
            print("other")

    @staticmethod
    def pick[T: ImplicitlyCopyable & Writable](x: T) -> T:
        comptime if T == Int:
            print("pick int")
        return x

    @staticmethod
    def count[T: Copyable](x: T, n: Int) -> Int:
        comptime if T == Int:
            if n == 0:
                return 0
            return 1 + S.count(x, n - 1)
        else:
            return -1

    @staticmethod
    def relay[T: Copyable](x: T):
        S.show(x)
        S.show(1.5)

    def ishow[T: Copyable](self, x: T):
        comptime if T == Int:
            print("instance int")
        else:
            print("instance other")

    def irelay[T: Copyable](self, x: T):
        self.ishow(x)


@fieldwise_init
struct B[U: ImplicitlyCopyable & Deinitable](Copyable):
    var u: Self.U

    @staticmethod
    def show[T: Copyable](x: T):
        comptime if T == Int:
            print("B int")
        else:
            print("B other")

    @staticmethod
    def make[T: Copyable](u: Self.U, x: T) -> B[Self.U]:
        comptime if T == Int:
            print("make int")
        else:
            print("make other")
        return B[Self.U](u)


def main():
    S.show(3)
    S.show(4)
    S.show(String("s"))
    print(S.pick(7))
    print(S.pick(2.5))
    print(S.count(1, 3))
    print(S.count(1.0, 3))
    S.relay(True)
    S.relay(2)
    var s = S()
    s.show(5)
    s.irelay(6)
    s.irelay("t")
    B[Int].show(3)
    B[Int].show("x")
    var b = B.make(1, "s")
    print(b.u)
