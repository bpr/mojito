struct Noisy(Copyable, Movable):
    var id: Int

    def __init__(out self, id: Int):
        self.id = id
        print("make", id)

    def __deinit__(deinit self):
        print("drop", self.id)


def scoped():
    var a = Noisy(1)
    var b = Noisy(2)
    print("mid", a.id, b.id)


def main():
    scoped()
    print("after")
