# A method's named `out` result: a callee-local slot the method returns,
# which no caller passes, on an instance, a consuming, a static, and a
# generic struct's method.
@fieldwise_init
struct Pair(Movable):
    var a: Int
    var b: Int

    def flip(deinit self, out result: Pair):
        result = Pair(self.b, self.a)

    def sum_with(self, other: Pair, out result: Int):
        result = self.a + self.b + other.a + other.b

    @staticmethod
    def unit(out result: Pair):
        result = Pair(1, 1)

    def early(self, flag: Bool, out result: String):
        result = String("late")
        if flag:
            result = String("early")
            return


struct Wrapper[T: Copyable & Deinitable]:
    var value: Self.T

    def __init__(out self, value: Self.T):
        self.value = value.copy()

    def listed(self, out result: List[Self.T]):
        result = List[Self.T]()
        result.append(self.value.copy())
        result.append(self.value.copy())


def main():
    var p = Pair(1, 2)
    print(p.sum_with(Pair(3, 4)))
    print(p.early(True), p.early(False))
    var q = p^.flip()
    print(q.a, q.b)
    var u = Pair.unit()
    print(u.a, u.b)
    var w = Wrapper(String("x"))
    var l = w.listed()
    print(len(l), l[0], l[1])
