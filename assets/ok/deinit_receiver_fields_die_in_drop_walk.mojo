# Pinned against Mojo 1.2.0.dev2026101705 (2026-10-23): a `deinit`
# parameter's fields die in the one drop walk, as upstream's
# `DestructorInsertion` destroys them. Every exit marks the receiver
# destroyed (`mark_destroyed`), so each field dies at its own last use, and
# a path that transfers the receiver whole (`self^.finish()`) hands every
# field still held to the new owner, which destroys the unused one at its
# entry. A field a pointer borrows dies after the pointer's last use, a
# second `deinit` parameter's fields die as the receiver's do, and a field
# last read before a `while` loop dies before the loop, while one read
# after it lives through it.
# Expected:
#   go 1 / del inner 1 / finish 2 / del inner 2
#   go 1 / del inner 1 / kept 2 / del inner 2
#   b 4 / del inner 4 / mid / p 3 / del inner 3 / end
#   swap 5
#   a 5 / del inner 5 / while 0 / while 1 / b 6 / del inner 6 / done
struct Inner:
    var id: Int

    def __init__(out self, id: Int):
        self.id = id

    def __deinit__(deinit self):
        print("del inner", self.id)


struct W:
    var a: Inner
    var b: Inner

    def __init__(out self, a: Int, b: Int):
        self.a = Inner(a)
        self.b = Inner(b)

    def finish(deinit self):
        print("finish", self.b.id)

    def go(deinit self, flag: Bool):
        print("go", self.a.id)
        if flag:
            self^.finish()
            return
        print("kept", self.b.id)

    def borrowed(deinit self):
        var p = Pointer(to=self.a)
        print("b", self.b.id)
        print("mid")
        print("p", p[].id)
        print("end")

    def loops(deinit self):
        print("a", self.a.id)
        var j = 0
        while j < 2:
            print("while", j)
            j += 1
        print("b", self.b.id)


struct Token(Movable):
    var id: Int

    def __init__(out self, id: Int):
        self.id = id


struct Box2(Movable):
    var t: Tuple[Token, Token]

    def __init__(out self, var t: Tuple[Token, Token]):
        self.t = t^

    def swap(deinit self, deinit other: Box2) -> Int:
        return self.t[0].id + other.t[1].id


def main():
    var w = W(1, 2)
    w^.go(True)
    var v = W(1, 2)
    v^.go(False)
    var u = W(3, 4)
    u^.borrowed()
    var a = Box2((Token(2), Token(9)))
    var b = Box2((Token(4), Token(3)))
    print("swap", a^.swap(b^))
    var l = W(5, 6)
    l^.loops()
    print("done")
