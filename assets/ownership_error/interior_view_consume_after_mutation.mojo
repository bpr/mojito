# expect: invalidated interior reference
# Consuming an aggregate that carries an interior-origin loan requires the
# origin to still be live (current Mojo's rule): mutating the owner stales
# the view's generation, so the later whole-variable move of the aggregate
# is a use of the invalidated interior reference. Link-free twin of moving
# a stale Span.
@fieldwise_init
struct Buf:
    var value: Int

    def view(ref self) -> Pointer[
        Int, origin_of(self)._get_owned_interior["items"]
    ]:
        return UnsafePointer(to=self.value).unsafe_origin_cast[
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
    var x = w^
    print("moved")
