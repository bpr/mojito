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

def may_raise(flag: Bool) raises:
    if flag:
        raise Error("boom")

def work(flag: Bool) raises:
    var p = Pair(Inner(1), Inner(2))
    var x = p.a^
    print(x.id)
    may_raise(flag)
    p.a = Inner(3)

def main():
    try:
        work(True)
    except e:
        print("caught")
