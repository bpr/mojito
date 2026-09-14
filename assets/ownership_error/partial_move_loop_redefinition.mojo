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
    for i in range(2):
        var p = Pair(Inner(i), Inner(i + 10))
        var x = p.a^
        print(x.id)
