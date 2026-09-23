# `Sized` spelled as a method on a specialized runtime pack: `b.__len__()` is
# the element count, the same answer `len(b)` gives. The clone's pack storage
# is compiler-private — there is no nominal `Tuple` to resolve the method onto
# — so both the VM and the native backend answer it from the static arity.
def sink[*Ts: Copyable](*b: *Ts) -> Int:
    return b.__len__()


def spread[*Ts: Copyable](*b: *Ts) -> Int:
    return b.__len__() + len(b)


def main():
    print(sink(1, True))
    print(sink())
    print(spread(1, True, 3.5))
