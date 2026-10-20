# expect: @explicit_destroy is not valid on `struct` with unconditional conformance to `Deinitable`
# `@explicit_destroy` on a struct that conforms to `Deinitable`
# unconditionally. Both compilers reject it, as the pin's note says: "Add a
# `not Deinitable` conformance or remove `@explicit_destroy`". Adding
# `Deinitable where False` to the conformance list makes the program valid.
@explicit_destroy("finish the ticket")
struct Ticket(Movable):
    var id: Int

    def __init__(out self, id: Int):
        self.id = id

    def finish(deinit self) -> Int:
        return self.id


def main():
    var ticket = Ticket(3)
    print(ticket^.finish())
