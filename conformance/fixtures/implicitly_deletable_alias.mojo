# `ImplicitlyDeletable` was upstream's spelling of `Deinitable` before the
# rename. The pin no longer knows the name ("use of unknown declaration
# 'ImplicitlyDeletable'"), and Mojito rejects it as an unknown trait too.
struct Handle(Movable, ImplicitlyDeletable where False):
    var id: Int

    def __init__(out self, id: Int):
        self.id = id

    def release(deinit self):
        print("released", self.id)

def main():
    var h = Handle(1)
    h^.release()
