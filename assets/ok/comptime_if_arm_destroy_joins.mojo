# The arms of a `comptime if` keyed on a parameter join as `if` branches do,
# so a linear value is destroyed on every arm or on none. A condition that
# folds without the parameters selects its one arm first. A transferred
# temporary receiver still moves the places inside it.
@explicit_destroy("close it")
struct R(Deinitable where False):
    var x: Int

    def __init__(out self, x: Int):
        self.x = x

    def close(deinit self):
        print("close", self.x)

trait Rel:
    def release(deinit self): ...

struct Handle(Movable, Rel):
    var id: Int

    def __init__(out self, id: Int):
        self.id = id

    def release(deinit self):
        print("release", self.id)

def both[T: AnyType]():
    var r = R(1)
    comptime if T == Int:
        print("int arm")
        r^.close()
    else:
        print("other arm")
        r^.close()

def diverging[T: AnyType]():
    var r = R(2)
    comptime if T == Int:
        r^.close()
        return
    print("fell through")
    r^.close()

def constant[T: AnyType]():
    var r = R(3)
    comptime if True:
        r^.close()
    comptime if T == Int:
        print("constant int")

def pk[T: Rel & Movable](var x: T) -> T:
    return x^

def through[T: Rel & Movable](var x: T):
    comptime if T == Int:
        print("never")
    pk(x^)^.release()

def main():
    both[Int]()
    both[String]()
    diverging[Int]()
    diverging[String]()
    constant[Int]()
    through(Handle(4))
