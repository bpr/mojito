# `not Trait` in a conformance list opts a struct out of the trait, as
# `Trait where False` does: a `not Deinitable` struct must be destroyed
# explicitly, a `not Movable` one stays where it is built, and an opted-out
# trait's requirements and defaults are not asked for (`not Hashable`,
# `not Writable`, or a user trait).


trait Named:
    def name(self) -> String:
        ...


struct Session(Movable, not Deinitable else "call close() instead"):
    var id: Int

    def __init__(out self, id: Int):
        self.id = id

    def close(deinit self):
        print("closed", self.id)


@fieldwise_init
struct Pinned(not Movable, not Hashable, not Writable, not Named):
    var value: Int


@fieldwise_init
struct Token(ImplicitlyCopyable, not Deinitable):
    var id: Int

    def spend(deinit self) -> Int:
        return self.id


def read(pinned: Pinned) -> Int:
    return pinned.value


def main():
    var session = Session(1)
    print(session.id)
    session^.close()
    var pinned = Pinned(2)
    print(read(pinned))
    var token = Token(3)
    var copy = token
    print(copy^.spend() + token^.spend())
