# expect: cannot consume indirect references to values
# A `mut` capture is an indirect reference to the enclosing binding: it
# cannot be consumed, even when the body writes it back.
def outer() -> String:
    var s = String("a")
    def inner() {mut s} -> String:
        var r = s^
        s = String("b")
        return r^
    var got = inner()
    return got + s

def main():
    print(outer())
