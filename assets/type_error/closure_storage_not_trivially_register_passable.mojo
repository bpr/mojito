# expect: does not conform to trait 'TrivialRegisterPassable'
# A closure's storage is trivially register-passable only when every
# capture is: a moved `String` capture is not.
def call[F: TrivialRegisterPassable & def(Int) -> Int](f: F, a: Int) -> Int:
    return f(a)


def main():
    var s = String("ab")

    def adds(a: Int) {var s} -> Int:
        return a + s.byte_length()

    print(call(adds, 5))
