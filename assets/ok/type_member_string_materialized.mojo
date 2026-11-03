# A `String` compile-time member read through a type parameter (`T.S`) is
# a fresh value its parameter constant materializes into storage, as a
# value parameter's read is: a borrow reads the storage, a binding or an
# owned argument takes the value, and a method call borrows it.


trait HasS:
    comptime S: String


struct A(HasS):
    comptime S = "own"

    def __init__(out self):
        pass


def take(var s: String):
    s += "!"
    print("take", s)


def look(s: String):
    print("look", s)


def show[T: HasS]():
    print(T.S)
    var t = T.S
    t += "?"
    print(t)
    take(T.S)
    look(T.S)
    print(T.S.byte_length())
    print(T.S + "x")


def main():
    show[A]()
