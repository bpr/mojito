struct Noisy(Movable):
    var id: Int

    def __init__(out self, id: Int):
        self.id = id

    def __init__(out self, *, deinit move: Self):
        self.id = move.id

    def __deinit__(deinit self):
        print("drop", self.id)


def main():
    var values = (Noisy(13), Noisy(14))
    var first = values[0]^
    print(first.id)
