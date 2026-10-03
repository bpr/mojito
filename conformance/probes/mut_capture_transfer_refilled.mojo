# A `mut` capture transferred away and written back. The pin rejects the `^`
# ("cannot consume indirect references to values"); Mojito prints `ab`.
# `docs/roadmap.md` 3.105. When Mojito rejects it, move this to
# `assets/type_error/`.
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
