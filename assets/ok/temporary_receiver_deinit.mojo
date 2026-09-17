# A method called on an owning temporary receiver destroys that temporary
# right after the call returns, running its `__deinit__` as the pinned Mojo
# does: the receiver is bound to a hidden slot whose last use is the call. A
# `var`/`deinit` receiver is consumed by the callee instead and destroyed
# there once. Generic and non-generic structs alike.
struct B:
    var x: Int

    def __init__(out self, x: Int):
        self.x = x

    def __deinit__(deinit self):
        print("deinit", self.x)

    def show(self):
        print("show", self.x)

    def get(self) -> Int:
        return self.x

    def consume(var self):
        print("consume", self.x)

    def finish(deinit self):
        print("finish", self.x)

    def twin(self) -> B:
        return B(self.x * 10)


struct G[T: Writable & Copyable & Deinitable]:
    var x: Self.T

    def __init__(out self, x: Self.T):
        self.x = x.copy()

    def __deinit__(deinit self):
        print("gdeinit", self.x)

    def show(self):
        print("gshow", self.x)


def make(x: Int) -> B:
    return B(x)


def main():
    print("a read method")
    B(1).show()
    print("b method result: the receiver dies before print runs")
    print(B(2).get())
    print("c consuming method")
    B(3).consume()
    print("d named destructor")
    B(4).finish()
    print("e chained temporaries")
    B(5).twin().show()
    print("f free-function result")
    make(6).show()
    print("g result bound")
    var t = B(7).twin()
    print("got", t.x)
    print("h generic struct")
    G(8).show()
    G(String("nine")).show()
    print("end")
