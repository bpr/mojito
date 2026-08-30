# A multi-origin-binder delegated correspondence: the field application
# (`var second: EntryCursor[Self.o2]`) records which caller binder each
# callee binder bound, so the delegated origin resolves through the right
# field — the returned reference loans only the second source.
@fieldwise_init
struct Pair(Copyable, Movable):
    var key: Int
    var value: Int

@fieldwise_init
struct EntryCursor[m: Bool, //, o: Origin[mut=m]]:
    var src: ref[o] Pair

    def current(self) -> ref[Self.o] Pair:
        return self.src

@fieldwise_init
struct TwoView[m1: Bool, m2: Bool, //, o1: Origin[mut=m1], o2: Origin[mut=m2]]:
    var first: EntryCursor[Self.o1]
    var second: EntryCursor[Self.o2]

    def key(self) -> ref[self.second.current().key] Int:
        return self.second.current().key

def main():
    var a = Pair(1, 10)
    var b = Pair(2, 20)
    ref ra = a
    ref rb = b
    var tv = TwoView(EntryCursor(ra), EntryCursor(rb))
    print(tv.key())
