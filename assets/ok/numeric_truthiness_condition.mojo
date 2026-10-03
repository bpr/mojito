# Bare truthiness of a numeric scalar in every condition position: `if`,
# `while`, the conditional expression, a comprehension filter, and `not`,
# over `Int`, `UInt`, `Float64`, and a width-one lane, including in a method
# and a keyed `def`.
struct Holder[T: AnyType]:
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def nonzero(self) -> Bool:
        if self.v:
            return True
        return False


def keyed[n: Int](x: Int) -> Int:
    if x:
        return n
    return 0


def main():
    var n = 3
    var u: UInt = 2
    var f: Float64 = 0.5
    var b8: UInt8 = 0
    if n:
        print("int")
    if u:
        print("uint")
    if f:
        print("float")
    if b8:
        print("u8")
    else:
        print("no u8")
    while n:
        n -= 1
    print(n, not n, not f, not 1, "t" if n else "f", "y" if 2.5 else "n")
    var xs = [i for i in range(4) if i]
    print(len(xs))
    print(Holder[Int](0).nonzero(), Holder[Int](5).nonzero())
    print(keyed[7](1), keyed[7](0))
