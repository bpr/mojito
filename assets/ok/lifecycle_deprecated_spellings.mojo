# Deprecated-compat pin: upstream still accepts `__del__` as a deprecated
# spelling of `__deinit__`; Mojito normalizes it at parse time. This fixture
# stays on the OLD spelling on purpose — remove it when upstream removes the
# alias. `ImplicitlyDeletable`, the other alias Mojito normalizes, is already
# gone from the pin, so it is a `mojito-only` conformance case instead.
@explicit_destroy("release the handle")
struct Handle(Movable, Deinitable where False):
    var id: Int

    def __init__(out self, id: Int):
        self.id = id

    def release(deinit self):
        print("released", self.id)

struct Noisy:
    var id: Int

    def __init__(out self, id: Int):
        self.id = id

    def __del__(deinit self):
        print("dropped", self.id)

def main():
    var h = Handle(1)
    h^.release()
    var n = Noisy(2)
    print("body")
