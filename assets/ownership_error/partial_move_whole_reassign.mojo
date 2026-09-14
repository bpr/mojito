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

def main():
    var p = Pair(Inner(1), Inner(2))
    var x = p.a^
    print(x.id)
    p = Pair(Inner(3), Inner(4))
    print(p.a.id)
