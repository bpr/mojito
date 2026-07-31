# expect: does not match the signature
# Self-origin resolution makes the requirement `Self.IterType[origin_of(self)]`
# resolve to the conforming struct's concrete member — here `List[Int]`. A
# conformer whose method returns a different type (`List[Bool]`) therefore fails
# conformance, proving the requirement resolved concretely rather than staying an
# unconstrained symbolic application.
trait HasIter:
    comptime IterType[mut: Bool, //, origin: Origin[mut=mut]]: AnyType

    def make(ref self) -> Self.IterType[origin_of(self)]:
        ...

@fieldwise_init
struct Holder(HasIter):
    comptime IterType[mut: Bool, //, origin: Origin[mut=mut]] = List[Int]
    var v: Int

    def make(ref self) -> List[Bool]:
        return [True]

def main():
    print(1)
