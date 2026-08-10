# A non-Movable (`Movable where False`) linear value stays destructible: a
# `deinit self` named destructor consumes it without an ownership move.
@explicit_destroy("release the handle")
struct Handle(Movable where False, Deinitable where False):
    var id: Int

    def __init__(out self, id: Int):
        self.id = id

    def release(deinit self):
        print("released", self.id)

def main():
    var h = Handle(7)
    h^.release()
