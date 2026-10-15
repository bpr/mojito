# A parameter only forwarded to a callee parameter the callee removes, by a
# call or a method call, is removed too, as upstream's `RemoveUnusedParams`
# walks its call graph callees first, even through an operator that could
# trap, whose operation the pin never emits for a parameter argument; a
# forward to a parameter the callee reads, a read of its own, or a forward
# around a recursive call keeps it.
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

def rec[n: Int](k: Int) -> Int:
    if k == 0:
        return leaf[n]()
    return rec[n](k - 1)

def main():
    print(mid[1]() + mid[2]())
    print(two_hop[1]() + two_hop[2]())
    print(expr_fwd[1]() + expr_fwd[2]())
    print(div_fwd[1]() + div_fwd[2]())
    print(tmid[Int]() + tmid[String]())
    print(mixed[1]() + mixed[2]())
    print(to_reader[1]() + to_reader[2]())
    print(method_fwd[1]() + method_fwd[2]())
    print(rec[1](2) + rec[2](2))
