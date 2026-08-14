# expect: through a 'ref' binding requires a place or origin-parameter referent
# A reference whose origin is a union of places has no single subtree base;
# Pointer(to=...) stays rejected over it.
@fieldwise_init
struct Pair:
    var a: Int
    var b: Int

    def pick(ref self, flag: Bool) -> ref[self.a, self.b] Int:
        if flag:
            return self.a
        return self.b

def main():
    var t = Pair(3, 4)
    ref r = t.pick(True)
    var p = UnsafePointer(to=r)
    print(p[])
