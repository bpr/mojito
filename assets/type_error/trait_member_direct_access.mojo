# A trait member is named through a conforming type or a bound, never through
# the trait itself, even to reach its default.
# expect: Direct access of trait members is not supported.
trait T:
    def req(self) -> Int:
        ...

    def d(self) -> Int:
        return self.req() + 1

@fieldwise_init
struct S(T):
    var x: Int

    def req(self) -> Int:
        return self.x

    def d(self) -> Int:
        return T.d(self) * 10

def main():
    print(S(3).d())
