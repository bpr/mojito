# A display copies its element, so a capturing lambda that owns a
# non-copyable value by transfer is not a storable display element.
# expect: ImplicitlyCopyable
struct Token(Movable):
    var id: Int

    def __init__(out self, id: Int):
        self.id = id

def main():
    var token = Token(4)
    var fns = [lambda (x: Int) {var token^} -> Int: x + token.id]
    print(fns[0](1))
