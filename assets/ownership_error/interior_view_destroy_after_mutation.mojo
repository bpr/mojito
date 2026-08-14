# expect: invalidated interior reference
# A named explicit destructor consumes its receiver, and consuming an
# aggregate carrying an interior-origin loan requires that origin to remain
# live: the receiver load of `w^.finish()` observes the generation staled
# by the owner mutation. (The consume channel here is the explicit-destroy
# receiver place, distinct from the whole-variable move twin.)
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

    def finish(deinit self):
        pass

def main():
    var b = Buf(3)
    var w = Wrap(b.view())
    b.grow()
    w^.finish()
    print("done")
