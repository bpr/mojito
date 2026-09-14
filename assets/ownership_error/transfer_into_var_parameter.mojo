# expect: use of uninitialized value 'a'
# requires: stdlib
# A `^` into an owned `var` parameter moves the value out of its source.
struct Thing(Movable):
    var s: String

    def __init__(out self, var s: String):
        self.s = s^

def consume(var t: Thing) -> Int:
    return t.s.byte_length()

def main():
    var a = Thing(String("seven"))
    var got: Int = consume(a^)
    print(got)
    print(a.s)
