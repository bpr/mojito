# expect: value passed to a mutable argument must be mutable
# A transferred (`^`) value is not a place, so it cannot bind a `mut` parameter.
struct Thing(Movable):
    var s: String

    def __init__(out self, var s: String):
        self.s = s^

def poke(mut t: Thing):
    t.s = String("poked")

def main():
    var a = Thing(String("seven"))
    poke(a^)
    print(a.s)
