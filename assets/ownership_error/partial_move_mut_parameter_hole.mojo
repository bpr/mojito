# expect: 'p.a' is uninitialized at return from this function
@fieldwise_init
struct Inner:
    var id: Int
    def __deinit__(deinit self):
        print("del", self.id)

@fieldwise_init
struct Pair:
    var a: Inner
    var b: Inner

def hole(mut p: Pair):
    var x = p.a^
    print(x.id)

def main():
    var p = Pair(Inner(1), Inner(2))
    hole(p)
    print(p.b.id)
