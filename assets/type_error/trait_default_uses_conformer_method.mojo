# A trait default is checked once, in its trait, over the trait's `Self`:
# a method only the conformer declares is not a member of that `Self`.
# expect: type '_Self' has no method 'extra'
trait T:
    def req(self) -> Int:
        ...

    def d(self) -> Int:
        return self.extra()

@fieldwise_init
struct S(T):
    var x: Int

    def req(self) -> Int:
        return self.x

    def extra(self) -> Int:
        return 7

def main():
    print(S(1).d())
