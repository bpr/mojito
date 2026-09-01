# expect: conflicts with live reference
# The reference delegated through a carrier parameter loans the carrier's
# source: mutating it while the reference lives is rejected. (Upstream's
# checker permits this mutation — Mojito's loan rule is the documented
# stricter subset.)
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
    a.key = 5
    print(k)
