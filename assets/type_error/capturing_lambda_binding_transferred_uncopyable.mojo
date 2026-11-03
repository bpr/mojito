# A lambda is its storage variable, so a binding copies it, and a lambda
# that owns a non-copyable value by transfer cannot be implicitly copied.
# expect: cannot be implicitly copied
struct Token(Movable):
    var id: Int

    def __init__(out self, id: Int):
        self.id = id

def main():
    var token = Token(4)
    var peek = lambda {var token^} -> Int: token.id
    print(peek())
