# The field form of a plain-origin view temporary assigned back over its
# source: `StringSpan(b.s)` borrows `b.s` itself. The pinned Mojo prints
# `abc`.
struct Box:
    var s: String
    def __init__(out self, var s: String):
        self.s = s^

def main():
    var b = Box("abc")
    b.s = String(StringSpan(b.s))
    print(b.s)
