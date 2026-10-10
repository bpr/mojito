# A trait default's parameter defaults are its trait member's: a conformer
# that inherits the default leaves a parameter out and gets the member's
# default, evaluated over the conformer's own instantiation of the member.
comptime TWO = 2


trait T:
    comptime K: Int

    def f(self, x: Int = 3, y: Int = TWO + 1) -> Int:
        return x + y + Self.K

    def g[n: Int](self, x: Int = n * TWO) -> Int:
        return x + Self.K

    def h(self, o: Optional[Int] = None, s: String = "q") -> Int:
        if o:
            return o.value() + Self.K + s.byte_length()
        return Self.K + s.byte_length()

    def k[*, m: Int = 3](self, x: Int = m + 10) -> Int:
        return x


struct A(T):
    comptime K = 10

    def __init__(out self):
        pass


struct G[w: Int](T):
    comptime K = Self.w

    def __init__(out self):
        pass


struct H(T):
    comptime K = 1

    def __init__(out self):
        pass

    def g[n: Int](self, x: Int = 0) -> Int:
        return x - 1


def use[U: T](u: U) -> Int:
    return u.f() + u.g[5]()


def main():
    print(A().f())
    print(A().f(1))
    print(A().f(y=100))
    print(A().g[4]())
    print(use(A()))
    print(G[5]().h())
    print(G[6]().h(3, "ab"))
    print(G[7]().g[2]())
    print(H().h(o=4))
    print(H().g[9]())
    print(A().k())
    print(A().k[m=7]())
