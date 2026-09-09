# Moving one field out of a struct (`p.a^`) while the parent is still used
# afterwards (`p.b`): the pinned Mojo rejects the transfer ("value 'p.a'
# cannot be consumed, because 'p' is used later"); Mojito tracks the moved
# field and destroys the retained one on its own.
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
    var p: Pair = Pair(Inner(1), Inner(2))
    var x: Inner = p.a^
    print("x =", x.id)
    print("b =", p.b.id)
