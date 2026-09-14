# Moving a field out (`p.a^`) and reinitializing it before any other part
# of `p` is touched keeps the whole value usable and destroyable, as upstream.
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
    var p = Pair(Inner(1), Inner(2))
    var x = p.a^
    print("x =", x.id)
    p.a = Inner(3)
    print("b =", p.b.id)
