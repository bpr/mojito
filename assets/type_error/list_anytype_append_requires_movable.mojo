# expect: no overload matches the supplied arguments
# `append` states `where conforms_to(T, Movable)` (upstream: "violated
# constraint"); a pinned element type cannot be appended.
@fieldwise_init
struct Pinned(Movable where False, Deinitable):
    var id: Int

def main():
    var xs = List[Pinned]()
    xs.append(Pinned(1))
