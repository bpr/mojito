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

def take(var p: Pair):
    var x = p.a^
    print(x.id)

def main():
    take(Pair(Inner(1), Inner(2)))
