# expect: field 'p.a' destroyed out of the middle of a value
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
