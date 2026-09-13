# A reference whose origin is a union of places under one owner lends a
# Pointer to the subtree of their deepest common base.
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
    var p = Pointer(to=r)
    print(p[])
