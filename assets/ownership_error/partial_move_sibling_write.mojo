# expect: value 'p.a' cannot be consumed, because 'p' is used later
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
    p.b = Inner(3)
