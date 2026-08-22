# Owned droppable closure captures join the drop set through the capture
# record's teardown thunk: a `{var}` String capture frees its buffer when
# the closure dies, a `{var}` struct capture runs its user destructor at
# closure-drop time, and reference captures of droppable values leave the
# enclosing drop order untouched.
struct Token(Movable, Deinitable):
    var id: Int

    def __init__(out self, id: Int):
        self.id = id

    def __deinit__(deinit self):
        print("token drop", self.id)

def main():
    var label = String("captured")
    var describe: def() capturing[_] -> Int = lambda {var label^} -> Int: len(label)
    print(describe())
    print(describe())

    var token = Token(7)
    var peek: def() capturing[_] -> Int = lambda {var token^} -> Int: token.id
    print(peek())
    print("before scope end")
