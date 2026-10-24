# A reference delegated through a carrier parameter names the carrier's
# source as its origin: the `ref` binding of it borrows nothing exclusively,
# so a write to the source while the reference lives runs, as at the pin.
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
