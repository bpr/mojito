# expect: conflicts with live reference
# The alias-typed field's recorded binder correspondence makes the delegated
# reference loan exactly the second source, so mutating it while the
# reference lives is rejected.
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

@fieldwise_init
struct Wrap[m1: Bool, m2: Bool, //, o1: Origin[mut=m1], o2: Origin[mut=m2]]:
    comptime tv_t = TwoView[Self.o1, Self.o2]
    var tv: Self.tv_t

    def key(self) -> ref[self.tv.key()] Int:
        return self.tv.key()

def main():
    var a = Pair(1, 10)
    var b = Pair(2, 20)
    ref ra = a
    ref rb = b
    var w = Wrap(TwoView(EntryCursor(ra), EntryCursor(rb)))
    ref k = w.key()
    b.key = 99
    print(k)
