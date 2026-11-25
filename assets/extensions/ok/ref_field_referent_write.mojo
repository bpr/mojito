# An assignment to a declared `ref` field writes its referent, as upstream
# loads the field's `!lit.ref` and stores through it, destroying the value
# it replaces: a plain store, one in a `mut self` method or through a `mut`
# parameter, a loop condition's own field rewritten in its body, and a
# copied source that stays independent of the referent.
struct Noisy(Movable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __del__(deinit self):
        print("del", self.v)


@fieldwise_init
struct IntHolder[o: MutOrigin]:
    var r: ref[Self.o] Int

    def set(mut self, v: Int):
        self.r = v


@fieldwise_init
struct BoolHolder[o: MutOrigin]:
    var r: ref[Self.o] Bool


@fieldwise_init
struct StrHolder[o: MutOrigin]:
    var r: ref[Self.o] String

    def set(mut self, v: String):
        self.r = v


@fieldwise_init
struct NoisyHolder[o: MutOrigin]:
    var r: ref[Self.o] Noisy

    def put(mut self, v: Int):
        self.r = Noisy(v)


def put(mut h: StrHolder, v: String):
    h.r = v


def main():
    var n = 4
    var ih = IntHolder(n)
    ih.r = 7
    print(n)
    ih.set(9)
    print(n)
    ih.r += 2
    print(n)

    var flag = True
    var bh = BoolHolder(flag)
    var turns = 0
    while bh.r:
        turns += 1
        bh.r = turns < 3
    print(turns, flag)

    var s = String("ab")
    var sh = StrHolder(s)
    var t = String("zz")
    sh.r = t
    t += "?"
    print(s, t)
    sh.set(String("q"))
    print(s)
    put(sh, String("w"))
    print(s)

    var d = Noisy(1)
    var dh = NoisyHolder(d)
    dh.r = Noisy(2)
    print("mid", d.v)
    dh.put(3)
    print("end", d.v)
