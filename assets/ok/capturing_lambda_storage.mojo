# Capturing lambdas build their closure's storage struct where they stand
# and call its method: an immediately invoked lambda is a direct call
# passing the storage, by value when it is register-passable and by address
# otherwise, and a lambda used as a value moves its storage into the
# closure, so a loop still snapshots per evaluation and each call through
# the value reads a register-passable storage's own copy. A moved capture
# moves once, into the storage.
struct Counter(ImplicitlyCopyable, Movable, TrivialRegisterPassable):
    var n: Int

    def __init__(out self, n: Int):
        self.n = n

    def bump(mut self) -> Int:
        self.n += 1
        return self.n


struct Tracked(Movable):
    var n: Int

    def __init__(out self, n: Int):
        self.n = n

    def __init__(out self, *, deinit move: Self):
        print("move", move.n)
        self.n = move.n


def transform[F: def(x: Int) -> Int](f: F, v: Int) -> Int:
    return f(v)

def twice[F: def() -> Int](f: F) -> Int:
    return f() * 10 + f()

def scalar(n: Int) -> Int:
    var k = n
    return (lambda (x: Int) {k} -> Int: x * k)(3)

def text(s: String) -> Int:
    var label = s
    return (lambda (x: Int) {var label} -> Int: x + label.byte_length())(1)

def appended() -> Int:
    var pieces: List[Int] = [1]
    (lambda (x: Int) {mut pieces}: pieces.append(x))(4)
    return len(pieces)

def moved(n: Int) -> Int:
    var x = n
    return (lambda {var x^} -> Int: x + 1)()

def generic(k: Int) -> Int:
    return (lambda [N: Int](x: Int) {var k} -> Int: x + N + k)[5](3)

def unrolled(k: Int) -> Int:
    var total = 0
    comptime for j in range(3):
        total = total + (lambda (x: Int) {var k} -> Int: x + j + k)(10)
    return total

def branch(s: String, flag: Bool) -> Int:
    return (lambda {var s} -> Int: s.byte_length())() if flag else 0

def inside(s: String) -> Int:
    def outer() {var s} -> Int:
        return (lambda {var s} -> Int: s.byte_length() + 1)()
    return outer()

def main():
    print(scalar(2), text("abc"), appended(), moved(4))
    print(generic(2), unrolled(2))
    print(branch("abc", True), branch("abc", False), inside("abc"))
    var i = 0
    var total = 0
    while i < 3:
        total = total + (lambda {var i} -> Int: i)()
        i = i + 1
    ref r = total
    print(total, (lambda {var r} -> Int: r + 1)())
    var label = String("abc")
    print(transform(lambda (x: Int) {var label} -> Int: x + label.byte_length(), 10))
    var c = Counter(4)
    print(twice(lambda {var c^} -> Int: c.bump()))
    var t = Tracked(3)
    print((lambda {var t^} -> Int: t.n)())
    var u = Tracked(5)
    def holder() {var u^} -> Int:
        return u.n
    print(holder())
