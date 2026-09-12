# A free type-pack def (`def show[*Ts: Writable](*args: *Ts)`) specializes over
# any checked argument, not only literals and direct constructions: a local, a
# constructed value, and an origin-bearing temporary (`Named("k", w)`, whose
# erased origin slot the clone spells as `_`) are typed by the checker and
# minted on the next discovery round. A stored bundled iterator binds a local
# and drives a loop, and a generic def called over an origin-bearing struct
# value stays on its abstract path (`next(it)` is VM-only: the generic `next`
# body has no native reference-result adapter).
from std.format._utils import Named
from std.collections import Set

@fieldwise_init
struct Tag(Writable, Copyable, Movable):
    var value: Int

    def write_to(self, mut writer: Some[Writer]):
        writer.write("<", self.value, ">")

def show[*Ts: Writable](*args: *Ts):
    comptime for i in range(Ts.length):
        print(args[i])

def ident[T: AnyType](x: T) -> Int:
    return 1

def main():
    var xs: List[Int] = [1, 2, 3]
    var it = xs.__iter__()
    for rest in it:
        print(rest)
    var st = Set[Int]()
    st.add(9)
    var si = st.__iter__()
    for member in si:
        print(member)
    var w = 7
    var x = 5
    show(Named("k", w), Tag(8), "lit", x)
    var n = Named("m", w)
    show(n)
    print(ident(n), ident(it))
    w += 1
    print(w)
