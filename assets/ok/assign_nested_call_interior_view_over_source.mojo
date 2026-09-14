# Only the outermost call's direct arguments count: the view feeds the inner
# `String(...)`, whose owned result is what `Box(...)` receives. The pinned
# Mojo prints `abc`.
struct Box:
    var s: String
    def __init__(out self, var s: String):
        self.s = s^

def main():
    var b = Box("abc  ")
    b = Box(String(b.s.rstrip()))
    print(b.s)
