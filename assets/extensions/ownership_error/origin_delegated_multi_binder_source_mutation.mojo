# expect: conflicts with live reference
# The multi-binder delegated origin loans exactly its resolved source:
# mutating it while the returned reference lives is rejected (mutating the
# OTHER source stays legal — see the ok twin).
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
    ref k = tv.key()
    b.key = 99
    print(k)
