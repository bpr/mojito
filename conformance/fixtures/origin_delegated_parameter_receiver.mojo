# Upstream's expression-origin ref return rooted at a bare carrier parameter
# (`ref[c.current().key]` with `c: EntryCursor`). Both compilers print 1
# (confirmed against the a79fbdf59f2 pin, 2026-09-01).
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

def main():
    var a = Pair(1, 10)
    var c = EntryCursor(Pointer(to=a))
    ref k = first_key(c)
    print(k)
