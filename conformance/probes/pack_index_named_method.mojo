# PROBE: a variadic struct's index-keyed method under a name other than
# `__getitem__` or `__getitem_param__`: `def item[i: Int](self) ->
# Self.Ts[i]` over `self.storage[i].copy()`, called as `b.item[1]()`.
#
# **Differs.** The pin runs it and prints `a`. Mojito stops with "type
# 'Bag$t2[y3:Inty6:String]' has no associated type 'element_types'": only
# the two accessor names unroll per element, and any other method's
# `Self.Ts[i]` result is rewritten to `Tuple`'s private `element_types`
# projection. Filed in `docs/roadmap.md` §3. When Mojito prints `a`,
# promote this file to `assets/ok/` with its manifest rows.
#
# Observed 2026-09-28 against `Mojo 1.2.0.dev2026092105 (e9569894)`.
#
# Run:    mojo run pack_index_named_method.mojo


struct Bag[*Ts: Copyable & Deinitable](Copyable, Movable):
    var storage: Tuple[*Self.Ts]

    def __init__(out self, var *args: *Self.Ts):
        self.storage = Tuple(*args^)

    def item[i: Int](self) -> Self.Ts[i]:
        return self.storage[i].copy()


def main():
    var b = Bag[Int, String](1, "a")
    print(b.item[1]())
