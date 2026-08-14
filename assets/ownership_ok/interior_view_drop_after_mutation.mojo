# Implicitly dropping a stale view stays legal: the consume-time interior
# liveness rule applies to consuming uses (moves, explicit destructors), not
# to a scope-end drop of an aggregate whose interior generation was staled
# after its last use.
@fieldwise_init
struct Buf:
    var value: Int

    def view(ref self) -> Pointer[
        Int, origin_of(self)._get_owned_interior["items"]
    ]:
        return UnsafePointer(to=self.value).origin_cast[
            origin_of(self)._get_owned_interior["items"]
        ]()

    def grow(mut self):
        self.value += 1

@fieldwise_init
struct Wrap[mut: Bool, //, origin: Origin[mut=mut]]:
    var data: Pointer[Int, Self.origin._get_owned_interior["items"]]

def main():
    var b = Buf(3)
    var w = Wrap(b.view())
    b.grow()
    print(b.value)
