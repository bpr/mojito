# Deprecated-compat pin: upstream still accepts `ImplicitlyDeletable` and
# `__del__` as deprecated spellings of `Deinitable`/`__deinit__`; Mojito
# normalizes them at parse time. This fixture stays on the OLD spellings on
# purpose — remove it when upstream removes the aliases.
@explicit_destroy("release the handle")
struct Handle(Movable, ImplicitlyDeletable where False):
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
