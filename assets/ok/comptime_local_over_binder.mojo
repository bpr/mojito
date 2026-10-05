# A local `comptime` binding over a generic declaration's binders is a
# parameter expression the template keeps symbolic, as the pin's `comptime`
# alias is: a value one (`comptime m = n + 1`, a pack's `Us.length`) reads at
# run time, keys a `comptime if`, sizes a vector, and bounds a `comptime
# for`; a type one (`comptime U = T`) annotates a local. Each body, a
# generic struct's method included, is served by its template.


def shifted[n: Int]() -> Int:
    comptime m = n + 1
    comptime k = m * 2
    comptime if k > 6:
        return k
    else:
        return -k


def counted[*Us: AnyType]() -> Int:
    comptime count = Us.length
    return count


def kept[T: Copyable & Writable](x: T) -> T:
    comptime U = T
    var y: U = x.copy()
    return y^


def lanes[n: Int]() -> Int:
    comptime width = n * 2
    var v = SIMD[DType.int64, width](1)
    return Int(v.reduce_add())


def summed[n: Int]() -> Int:
    comptime bound = n + 1
    var total = 0
    comptime for i in range(bound):
        total += i
    return total


struct Offset[n: Int]:
    var x: Int

    def __init__(out self, x: Int):
        self.x = x

    def get(self) -> Int:
        comptime k = Self.n + 1
        comptime if k > 3:
            return self.x + k
        return self.x - k


def main():
    print(shifted[3](), shifted[1]())
    print(counted[Int, String](), counted[Int]())
    print(kept(5), kept(String("s")))
    print(lanes[2]())
    print(summed[3]())
    print(Offset[5](10).get(), Offset[1](10).get())
