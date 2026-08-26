# `List`'s element type is bounded by `AnyType` (upstream 2026-08); the
# move-requiring APIs state their own `Movable` requirement, so a pinned
# (non-Movable) element type still permits an empty list.
@fieldwise_init
struct Pinned(Movable where False, Deinitable):
    var id: Int

def main():
    var xs = List[Pinned]()
    print(len(xs))
