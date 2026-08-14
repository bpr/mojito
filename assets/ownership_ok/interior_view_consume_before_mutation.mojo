# Consuming a live view is legal: the explicit destructor runs while the
# interior generation is still current, and the owner mutates freely once
# the view is gone.
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
    w^.finish()
    b.grow()
    print(b.value)
