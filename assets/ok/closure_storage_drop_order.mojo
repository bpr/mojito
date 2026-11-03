# Owned captures drop with their closure's storage: a `{var}` String
# capture frees its buffer when the storage local dies, and a `{var}`
# struct capture runs its user destructor when the storage passed straight
# to an `F: def() -> Int` binder dies after the call. Reference captures of
# droppable values leave the enclosing drop order untouched.
struct Token(Movable, Deinitable):
    var id: Int

    def __init__(out self, id: Int):
        self.id = id

    def __deinit__(deinit self):
        print("token drop", self.id)

def call[F: def() -> Int](f: F) -> Int:
    return f()

def main():
    var label = String("captured")
    var describe = lambda {var label^} -> Int: label.byte_length()
    print(describe())
    print(describe())

    var token = Token(7)
    print(call(lambda {var token^} -> Int: token.id))
    print("before scope end")
