# Upstream's expression-origin ref return on a delegating wrapper: the origin
# is spelled as the delegated call projection itself
# (`ref [self.cursor.current().key]`), and the body returns that projection.
# Both compilers print 7 (confirmed against the a79bdf59f2 pin, 2026-08-28).
# The pin requires the qualified `Self.o` binder spelling in the callee's
# origin clause; Mojito accepts both `Self.o` and the bare binder.
@fieldwise_init
struct Pair(Copyable, Movable):
    var key: Int
    var value: Int

@fieldwise_init
struct EntryCursor[m: Bool, //, o: Origin[mut=m]]:
    var src: Pointer[Pair, Self.o]

    def current(self) -> ref[Self.o] Pair:
        return self.src[]

@fieldwise_init
struct KeyView[m: Bool, //, o: Origin[mut=m]]:
    var cursor: EntryCursor[Self.o]

    def key(self) -> ref[self.cursor.current().key] Int:
        return self.cursor.current().key

def main():
    var p = Pair(7, 70)
    var kv = KeyView(EntryCursor(Pointer(to=p)))
    print(kv.key())
