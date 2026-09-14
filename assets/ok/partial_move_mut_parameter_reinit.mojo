# A field moved out of a `mut` parameter must be written back before the
# callee returns; with the write-back the caller's value is whole again.
@fieldwise_init
struct Inner:
    var id: Int
    def __deinit__(deinit self):
        print("del", self.id)

@fieldwise_init
struct Pair:
    var a: Inner
    var b: Inner

def fix(mut p: Pair):
    var x = p.a^
    print(x.id)
    p.a = Inner(7)

def main():
    var p = Pair(Inner(1), Inner(2))
    fix(p)
    print(p.a.id, p.b.id)
