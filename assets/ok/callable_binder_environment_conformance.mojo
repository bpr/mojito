# A callable binder solved by a thin function keeps `thin` in its solution,
# so `conforms_to` answers for the function's type, not the contract's: a
# thin function and a closure over trivial captures are trivially
# register-passable, a closure over a `String` is neither.


def inc(x: Int) -> Int:
    return x + 1


def probe[F: Copyable & def(Int) -> Int](f: F):
    comptime if conforms_to(F, TrivialRegisterPassable):
        print("trivial")
    else:
        print("not trivial")
    comptime if conforms_to(F, RegisterPassable):
        print("register")
    else:
        print("not register")
    print(f(1))


def main():
    probe(inc)
    var k = 3

    def add(x: Int) {var k} -> Int:
        return x + k

    probe(add)
    var s = String("ab")

    def add_len(x: Int) {var s} -> Int:
        return x + s.byte_length()

    probe(add_len)
