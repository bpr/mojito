# A starred remainder borrows a temporary subject, which lives until the
# remainder's last use and then dies, in a target and in a `match`.
@fieldwise_init
struct D(Copyable):
    var v: Int

    def __deinit__(deinit self):
        print("del", self.v)


def make() -> List[D]:
    var xs: List[D] = []
    xs.append(D(1))
    xs.append(D(2))
    xs.append(D(3))
    return xs^


def main() raises:
    var [*rest] = make()
    print("before")
    print(len(rest), rest[0].v)
    print("after")
    match make():
        case [_, *mid, _]:
            print("mid", mid[0].v)
        case _:
            pass
    print("end")
