# expect: 'x' is borrowed mutably and also used at the same call
# A `@fieldwise_init` construction is its synthesized `__init__`'s call, whose
# `ref` parameters bind the arguments' places, so passing one variable to two
# mutable `ref` fields aliases a mutable borrow, as the written
# `ref [Self.o]` constructor's call does.
@fieldwise_init
struct Two[o: MutOrigin]:
    var a: ref[Self.o] Int
    var b: ref[Self.o] Int


def main():
    var x = 1
    var t = Two(x, x)
    t.a = 5
    print(t.b)
