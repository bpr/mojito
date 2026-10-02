# Probe: does a method with a parameter of its own keep the loan of a value it
# stores a copy of?
#
# `put[U]` runs as a per-call clone over the receiver's loan-carrying
# argument, which the clone spells with an origin binder. Its store of
# `value.copy()` names no argument expression, so the loan must come from the
# stored type.
#
# Pin (2026-09-21): prints `3 1 8`.
# Mojito: stops at run time ("checked nominal subscript receiver is None"):
# the list of pointers does not keep `xs` alive, which is destroyed after its
# last naming.
struct Slot[T: Copyable & Deinitable](Movable):
    var items: List[Self.T]

    def __init__(out self):
        self.items = List[Self.T]()

    def put[U: Copyable & Deinitable](mut self, value: Self.T, tag: U) -> U:
        self.items.append(value.copy())
        return tag.copy()


def main():
    var xs: List[Int] = [7, 8, 9]
    var s = Slot[Pointer[List[Int], ImmOrigin(origin_of(xs))]]()
    var p: Pointer[List[Int], ImmOrigin(origin_of(xs))] = Pointer(to=xs)
    var t = s.put(p, 3)
    print(t, len(s.items), s.items[0][][1])
