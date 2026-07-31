# Self-origin resolution for a parameterized associated type. `HasIter` requires
# an origin-parameterized `IterType[origin]` and a method returning
# `Self.IterType[origin_of(self)]`. On the abstract trait method there is no bound
# `self`, so the origin argument is the symbolic self-origin; when `Holder`
# conforms, that application resolves concretely (the origin erases, like a
# pointer origin) so `Holder.make` satisfying the requirement resolves the member
# to the concrete `List[Int]`. Without self-origin resolution the requirement
# stayed a symbolic zero-argument application and conformance failed.
trait HasIter:
    comptime IterType[mut: Bool, //, origin: Origin[mut=mut]]: AnyType

    def make(ref self) -> Self.IterType[origin_of(self)]:
        ...

@fieldwise_init
struct Holder(HasIter):
    comptime IterType[mut: Bool, //, origin: Origin[mut=mut]] = List[Int]
    var v: Int

    def make(ref self) -> List[Int]:
        return [self.v]

def main():
    var h = Holder(7)
    var xs = h.make()
    print(len(xs))
