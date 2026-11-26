# expect: atomicrmw instructions cannot be unordered
# A read-modify-write cannot be `UNORDERED`, as LLVM's verifier rules.
from std.atomic import Atomic, Ordering


def main():
    var a = Atomic[Int](1)
    print(a.fetch_add[ordering=Ordering.UNORDERED](2))
