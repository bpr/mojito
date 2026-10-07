# A variadic struct's method whose collector is the struct's own pack
# (`*b: *Self.Ts`): called on a closed instance, through a `var`, as a static
# method, and from a pack-keyed `def` that spreads its own collector into it.
# Each forwarding `def` is served by its template.


struct V[*Ts: Writable & Movable]:
    def __init__(out self):
        pass

    def put(self, *b: *Self.Ts):
        print("put", b.__len__())

    def own(self, var *b: *Self.Ts):
        print("own", b.__len__())

    def each(self, *b: *Self.Ts):
        comptime for i in range(b.__len__()):
            print(i, b[i])

    def again(self, *b: *Self.Ts):
        self.each(*b)

    @staticmethod
    def count(*b: *Self.Ts) -> Int:
        return b.__len__()


def fwd[*Ts: Writable & Movable](*a: *Ts):
    V[*Ts]().put(*a)
    V[*Ts]().each(*a)


def fwd_own[*Ts: Writable & Movable](var *a: *Ts):
    V[*Ts]().own(*a^)


def main():
    fwd(1, "x")
    fwd(2.5)
    fwd_own(1, "y")
    var v = V[Int, String]()
    v.put(3, "z")
    v.again(4, "w")
    print(V[Int, String].count(5, "v"))
