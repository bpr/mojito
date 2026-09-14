# expect: value 'p.a' cannot be consumed, because 'p' is used later
# Moving one field out (`p.a^`) and reading a sibling (`p.b`) later leaves
# the whole value with a hole it cannot be destroyed through; upstream
# rejects it too.
@fieldwise_init
struct Inner:
    var id: Int
    def __deinit__(deinit self):
        print("del", self.id)

@fieldwise_init
struct Pair:
    var a: Inner
    var b: Inner

def main():
    var p: Pair = Pair(Inner(1), Inner(2))
    var x: Inner = p.a^
    print("x =", x.id)
    print("b =", p.b.id)
