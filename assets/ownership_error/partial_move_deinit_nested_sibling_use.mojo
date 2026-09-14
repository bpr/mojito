# expect: value 'self.p.a' cannot be consumed, because 'self' is used later
@fieldwise_init
struct Inner:
    var id: Int
    def __deinit__(deinit self):
        print("del", self.id)

@fieldwise_init
struct Pair:
    var a: Inner
    var b: Inner

struct Outer:
    var p: Pair
    def __init__(out self, var p: Pair):
        self.p = p^
    def __deinit__(deinit self):
        var x = self.p.a^
        print("deinit", x.id, self.p.b.id)

def main():
    var o = Outer(Pair(Inner(1), Inner(2)))
