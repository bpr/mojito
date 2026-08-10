struct Noisy(Movable):
    var id: Int

    def __init__(out self, id: Int):
        self.id = id

    def __init__(out self, *, deinit move: Self):
        self.id = move.id

    def __deinit__(deinit self):
        print("drop", self.id)

@fieldwise_init
struct Holder:
    var storage: Tuple[Noisy, Noisy]

def main():
    var holder = Holder((Noisy(1), Noisy(2)))
    print("use", holder.storage[0].id)
    var keep_alive = holder.storage[1].id
