# A `nodebug` function's `mut` parameter passed a `ref` field (`bump(v.src)`)
# is inlined with the parameter bound to the handle the field stores, so the
# write lands in the referent.
struct View[o: MutOrigin]:
    var src: ref[o] Int
    var index: Int

    def __init__(out self, ref[Self.o] src: Int, index: Int):
        self.src = src
        self.index = index

@always_inline("nodebug")
def bump(mut x: Int):
    x += 1

def main():
    var n = 1
    var v = View(n, 0)
    bump(v.src)
    bump(v.src)
    print(v.index)
    print(n)
