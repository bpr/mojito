# A field-less element read through collection storage touches no memory,
# so the zero-size allocation behind it is never a dangling dereference.


struct E(Copyable):
    def __init__(out self):
        pass

    def me(self) -> Int:
        return 1


struct R(TrivialRegisterPassable):
    def __init__(out self):
        pass

    def me(self) -> Int:
        return 2


def main():
    var es = List[E]()
    es.append(E())
    var rs = List[R]()
    rs.append(R())
    rs.append(R())
    print(es[0].me(), rs[1].me(), len(rs))
    print(Optional(E()).value().me(), Optional(R()).value().me())
