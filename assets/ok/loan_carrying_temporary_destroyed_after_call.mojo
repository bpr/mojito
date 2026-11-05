# A memory-only temporary that carries a loan lives in its call-result slot,
# which carries the loan: at a read parameter it is destroyed once the call
# returns, before the enclosing `print` (`del 7`, then `7`), and an owning
# parameter takes the slot, so only the callee destroys it.
@fieldwise_init
struct Holder[m: Bool, //, o: Origin[mut=m]](Movable):
    var src: Pointer[Int, Self.o]

    def __deinit__(deinit self):
        print("del", self.src[])


def look(h: Holder) -> Int:
    return h.src[]


def take(var h: Holder) -> Int:
    return h.src[]


def main():
    var local = 7
    print(look(Holder(Pointer(to=local))))
    print(take(Holder(Pointer(to=local))))
