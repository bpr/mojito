# A value read of a declared `ref` field loads the handle the field stores
# and reads the referent through it, as upstream loads the field's
# `!lit.ref` and then loads through it: an operator's receiver, a method's
# `self.r`, a by-value or owned argument, a copy into a binding, and a loop
# condition all see the referent, and a copy leaves the referent unchanged.
@fieldwise_init
struct IntHolder[o: MutOrigin]:
    var r: ref[Self.o] Int

    def plus(self) -> Int:
        return self.r + 1

@fieldwise_init
struct StrHolder[o: MutOrigin]:
    var r: ref[Self.o] String

@fieldwise_init
struct FlagHolder[o: MutOrigin]:
    var r: ref[Self.o] Bool

def by_val(x: Int) -> Int:
    return x * 10

def by_owned(var x: Int) -> Int:
    x += 1
    return x

def by_owned_str(var s: String) -> String:
    s += "?"
    return s^

def main():
    var n = 4
    var ih = IntHolder(n)
    print(ih.r + 1, ih.r == 4, -ih.r)
    print(ih.plus())
    print(by_val(ih.r), by_owned(ih.r), n)
    var s = String("ab")
    var sh = StrHolder(s)
    print(by_owned_str(sh.r), sh.r + "c")
    var t = sh.r
    t += "!"
    print(t, s)
    var flag = True
    var fh = FlagHolder(flag)
    var turns = 0
    while fh.r and turns < 3:
        turns += 1
    print(turns, not fh.r)
