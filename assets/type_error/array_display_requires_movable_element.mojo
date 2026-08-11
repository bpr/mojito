# List-literal construction moves each element into the array, so a declared
# conditional `Movable` opt-out on the element rejects the display.
# expect: Movable
struct Pinned(Movable where False):
    var id: Int

    def __init__(out self, id: Int):
        self.id = id

def main():
    var a: Array[Pinned, 1] = [Pinned(1)]
    print(a[0].id)
