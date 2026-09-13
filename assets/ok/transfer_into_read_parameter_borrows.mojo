# A `^` into a read parameter or a read receiver lends its place: the source
# stays live, and it is destroyed at its own last use.
struct Thing(Movable):
    var s: String

    def __init__(out self, var s: String):
        self.s = s^

    def __del__(deinit self):
        print("del", self.s)

    def size(self) -> Int:
        return self.s.byte_length()

def consume(t: Thing) -> Int:
    print("in consume")
    return t.s.byte_length()

def used_after():
    var a = Thing(String("seven"))
    var got = consume(a^)
    print("after call", got)
    print(a.s)
    print("end of used_after")

def unused_after():
    var a = Thing(String("other"))
    var got = consume(a^)
    print("after call", got)
    print("end of unused_after")

def receiver():
    var a = Thing(String("eight"))
    print(a^.size())
    print(a.s)

def main():
    used_after()
    unused_after()
    receiver()
