# Upstream's named origins are `std.origin` values built from a raw
# `!lit.origin` attribute: each resolves through its declaration in an
# origin slot, through an alias of it, applied when generic, and as a value.
from std.origin import OriginSet, ImmutAnyOrigin

comptime MyO = MutUntrackedOrigin


struct S[mut: Bool, //, o: Origin[mut=mut]](TrivialRegisterPassable):
    def __init__(out self):
        pass


def show[mut: Bool, //, o: Origin[mut=mut]](s: S[o]):
    print(mut)


def main():
    show(S[ImmStaticOrigin]())
    show(S[UntrackedOrigin[mut=True]]())
    show(S[ImmUntrackedOrigin]())
    show(S[MutAnyOrigin]())
    show(S[ImmutAnyOrigin]())
    show(S[AnyOrigin[mut=True]]())
    show(S[UnsafeAnyOrigin[mut=False]]())
    show(S[MutUnsafeAnyOrigin]())
    show(S[MyO]())
    print(ImmStaticOrigin.mut, MutAnyOrigin.mut)
    comptime L = ImmStaticOrigin
    print(L.mut)
