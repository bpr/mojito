# A two-origin-binder delegated correspondence resolves through the
# applied field binder. Both compilers print 2 (confirmed against the
# a79fbdf59f2 pin, 2026-08-30).
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
struct TwoView[m1: Bool, m2: Bool, //, o1: Origin[mut=m1], o2: Origin[mut=m2]]:
    var first: EntryCursor[Self.o1]
    var second: EntryCursor[Self.o2]

    def key(self) -> ref[self.second.current().key] Int:
        return self.second.current().key

def main():
    var a = Pair(1, 10)
    var b = Pair(2, 20)
    var tv = TwoView(EntryCursor(Pointer(to=a)), EntryCursor(Pointer(to=b)))
    print(tv.key())
