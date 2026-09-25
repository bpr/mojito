# `@explicit_destroy` on a struct that conforms to `Deinitable`
# unconditionally. The pinned Mojo rejects it: "@explicit_destroy is not valid
# on `struct` with unconditional conformance to `Deinitable`" (note: "Add a
# `not Deinitable` conformance or remove `@explicit_destroy`"). Mojito accepts
# it, prints 3, and treats the value as linear. Adding `Deinitable where
# False` to the conformance list makes both compilers agree.
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
