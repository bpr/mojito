# A delegated origin clause rooted at a bare carrier PARAMETER (upstream's
# `ref[c.current().key]` on `c: EntryCursor`): the callee's struct binder has
# no record or enclosing binder to name it, so the returned reference is
# declared to borrow the carrier, and the call site resolves that to the
# sources the carrier holds — the reference loans `a`, not `c`, and re-roots
# at `a`. Both compilers print 1 (pin a79fbdf59f2, 2026-09-01). The
# ref-field carrier encoding resolves the same way.
@fieldwise_init
struct Pair(Copyable, Movable):
    var key: Int
    var value: Int

@fieldwise_init
struct EntryCursor[m: Bool, //, o: Origin[mut=m]]:
    var src: Pointer[Pair, Self.o]

    def current(self) -> ref[Self.o] Pair:
        return self.src[]

def first_key(c: EntryCursor) -> ref[c.current().key] Int:
    return c.current().key

@fieldwise_init
struct RefCursor[m: Bool, //, o: Origin[mut=m]]:
    var src: ref[o] Pair

    def current(self) -> ref[Self.o] Pair:
        return self.src

def ref_key(c: RefCursor) -> ref[c.current().key] Int:
    return c.current().key

def main():
    var a = Pair(1, 10)
    var c = EntryCursor(Pointer(to=a))
    ref k = first_key(c)
    print(k)
    var b = Pair(2, 20)
    ref rb = b
    var rc = RefCursor(rb)
    ref j = ref_key(rc)
    print(j)
