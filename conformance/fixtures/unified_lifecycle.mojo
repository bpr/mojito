struct Token(Copyable, Movable):
    var value: Int

    def __init__(out self, value: Int):
        self.value = value

    def __init__(out self, *, copy: Self):
        self.value = copy.value

    def __init__(out self, *, deinit move: Self):
        print("move")
        self.value = move.value

def main():
    var original = Token(7)
    var copied = Token(copy=original)
    var moved = copied^
    print(original.value, moved.value)
