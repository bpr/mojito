# expect: '_subtree' origins are supported only as Pointer origin arguments
# First-pass surface: `._subtree` is accepted in Pointer origin arguments
# and origin_cast targets; a `ref [...]` result clause rejects it.
@fieldwise_init
struct Buf:
    var value: Int

    def peek(ref self) -> ref[origin_of(self)._subtree] Int:
        return self.value

def main():
    var b = Buf(3)
    print(b.peek())
