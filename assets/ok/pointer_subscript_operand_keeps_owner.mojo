# A pointer subscript passed by address reads through a pointer holding its
# owner's loans, so the owner the pointer borrows outlives the call even when
# the call is the owner's last use.


struct E(Movable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __deinit__(deinit self):
        print("del", self.v)

    def show(self):
        print("show", self.v)


def take(e: E):
    print("take", e.v)


def main():
    var ls = List[E]()
    ls.append(E(7))
    ls.append(E(8))
    take(ls.unsafe_ptr()[1])
    ls.unsafe_ptr()[0].show()
    print("next")
