# `Variant[...](init_with=factory)` is upstream's placement constructor: the
# zero-parameter factory's result type selects the alternative and lands
# directly in the payload storage, so a `Movable where False` alternative can
# be constructed; a capturing lambda factory works too.
from std.utils import Variant

@fieldwise_init
struct Pinned(Movable where False, Writable):
    var v: Int

    def write_to(self, mut writer: Some[Writer]):
        writer.write("Pinned(", self.v, ")")

def make() -> Pinned:
    return Pinned(7)

def main():
    var p = Variant[Pinned, Int](init_with=make)
    print(p.isa[Pinned](), p.isa[Int]())
    print(p[Pinned].v)
    var base = 40
    var q = Variant[Int, String](init_with=lambda () -> Int: base + 2)
    print(q.isa[Int](), q[Int])
    print(q[Int] + 1)
