# A delegated origin clause through an ALIAS-typed field: the field's origin
# application is recorded from the comptime alias body (`TwoView[Self.o1,
# Self.o2]`), monomorphic and parameterized alike, so a two-binder delegation
# resolves to the right enclosing binder instead of the single-binder
# fallback. Upstream prints 2 for the same program (pin 2026-09-01). Subset
# precision limit: construction-time field origins are recorded per top-level
# field, so the caller-side loan of a reference delegated through a nested
# multi-binder carrier covers every source that carrier holds (both `a` and
# `b` here), not only the resolved one.
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

@fieldwise_init
struct WrapApplied[m1: Bool, m2: Bool, //, o1: Origin[mut=m1], o2: Origin[mut=m2]]:
    # A parameterized alias: its own binder `x` is bound by the application
    # (`Self.o2`), while the body names this struct's `o1` directly — the
    # callee's second binder resolves to `o1`, its first to `o2`.
    comptime view_t[a: Bool, //, x: Origin[mut=a]] = TwoView[x, Self.o1]
    var tv: Self.view_t[o2]

    def key(self) -> ref[self.tv.key()] Int:
        return self.tv.key()

def main():
    var a = Pair(1, 10)
    var b = Pair(2, 20)
    ref ra = a
    ref rb = b
    var w = Wrap(TwoView(EntryCursor(ra), EntryCursor(rb)))
    print(w.key())
    var x = WrapApplied(TwoView(EntryCursor(rb), EntryCursor(ra)))
    print(x.key())
