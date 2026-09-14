# expect: field 'self.p.a' destroyed out of the middle of a value
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
        print("deinit", x.id)

def main():
    var o = Outer(Pair(Inner(1), Inner(2)))
