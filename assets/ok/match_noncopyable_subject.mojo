# A bare binding and an `as` binding borrow a non-copyable subject, a place
# or a temporary, without copying it; the temporary is destroyed after the
# match.
struct Big(Movable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __deinit__(deinit self):
        print("del", self.v)


def make() -> Big:
    return Big(3)


def main():
    var b = Big(1)
    match b:
        case x:
            print("bare", x.v)
    match b:
        case {v=1} as whole:
            print("whole", whole.v)
        case _:
            print("other")
    match make():
        case x:
            print("temp", x.v)
    print("end")
