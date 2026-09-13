# `ImplicitlyDeletable` was upstream's spelling of `Deinitable` before the
# rename; Mojito still normalizes it at parse time, and the pin no longer
# knows the name at all.
struct Handle(Movable, ImplicitlyDeletable where False):
    var id: Int

    def __init__(out self, id: Int):
        self.id = id

    def release(deinit self):
        print("released", self.id)

def main():
    var h = Handle(1)
    h^.release()
