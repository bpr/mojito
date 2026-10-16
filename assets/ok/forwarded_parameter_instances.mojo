# A parameter only forwarded to a callee parameter the callee removes, by a
# call, a method call, or a subscript call, is removed too, as upstream's `RemoveUnusedParams`
# walks its call graph callees first, even through an operator that could
# trap, whose operation the pin never emits for a parameter argument, so
# that `10 // n` at `n == 0` fails nothing, and
# to a parameter only a default the call supplies reads, and one only
# passed as itself to a self-recursive call, by a call or a method call;
# a forward to a parameter the callee reads, or a read of its own, keeps it.
def leaf[n: Int]() -> Int:
    return 1

def mid[n: Int]() -> Int:
    return leaf[n]()

def two_hop[n: Int]() -> Int:
    return mid[n]()

def expr_fwd[n: Int]() -> Int:
    return leaf[n + 1]()

def div_fwd[n: Int]() -> Int:
    return leaf[10 // n]()

def div_outer[n: Int]() -> Int:
    return div_fwd[10 // n]()

def div_kept[n: Int]() -> Int:
    return leaf[10 // n]() + n

def dleaf[a: Int, b: Int = a]() -> Int:
    return b

def dmid[x: Int]() -> Int:
    return dleaf[x, 2]()

def tleaf[T: AnyType]() -> Int:
    return 3

def tmid[T: AnyType]() -> Int:
    return tleaf[T]()

def mixed[n: Int]() -> Int:
    return leaf[n]() + n

def reader[n: Int]() -> Int:
    return n

def to_reader[n: Int]() -> Int:
    return reader[n]()

@fieldwise_init
struct Holder(Copyable):
    var v: Int

    def get[n: Int](self) -> Int:
        return self.v

def method_fwd[n: Int]() -> Int:
    return Holder(4).get[n]()

@fieldwise_init
struct Grid(Copyable):
    var v: Int

    def __getitem_param__[i: Int](self) -> Int:
        return self.v

@fieldwise_init
struct Row(Copyable):
    var v: Int

    def __getitem_param__[i: Int](self) -> Int:
        return self.v + i

struct Static[n: Int]:
    @staticmethod
    def get() -> Int:
        return 7

def static_div_fwd[n: Int]() -> Int:
    return Static[10 // n].get()

def subscript_div_fwd[n: Int]() -> Int:
    return Grid(5)[10 // n]

def subscript_fwd[n: Int]() -> Int:
    return Grid(5)[n]

def subscript_expr_fwd[n: Int]() -> Int:
    return Grid(7)[n + 1]

def subscript_reader[n: Int]() -> Int:
    return Row(1)[n]

def rec[n: Int](k: Int) -> Int:
    if k == 0:
        return leaf[n]()
    return rec[n](k - 1)

def rec_reader[n: Int](k: Int) -> Int:
    if k == 0:
        return n
    return rec_reader[n](k - 1)

@fieldwise_init
struct Countdown(Copyable):
    var v: Int

    def down[n: Int](self, k: Int) -> Int:
        if k == 0:
            return self.v
        return self.down[n](k - 1)

def main():
    print(mid[1]() + mid[2]())
    print(two_hop[1]() + two_hop[2]())
    print(expr_fwd[1]() + expr_fwd[2]())
    print(div_fwd[0]() + div_fwd[2]())
    print(div_outer[0]() + div_outer[3]())
    print(div_kept[0]())
    print(dmid[1]() + dmid[2]())
    print(tmid[Int]() + tmid[String]())
    print(mixed[1]() + mixed[2]())
    print(to_reader[1]() + to_reader[2]())
    print(method_fwd[1]() + method_fwd[2]())
    print(subscript_fwd[1]() + subscript_fwd[2]())
    print(subscript_div_fwd[0]() + subscript_div_fwd[1]())
    print(static_div_fwd[0]() + static_div_fwd[1]())
    print(subscript_expr_fwd[1]() + subscript_expr_fwd[2]())
    print(subscript_reader[1]() + subscript_reader[2]())
    print(rec[1](2) + rec[2](2))
    print(rec_reader[1](2) + rec_reader[2](2))
    print(Countdown(3).down[1](2) + Countdown(3).down[2](2))
