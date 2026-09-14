# A view of `b.s`'s owned bytes does not lie under the sibling field
# `b.t`, so the assignment does not alias. The pinned Mojo prints `abc`.
struct Box:
    var s: String
    var t: String
    def __init__(out self, var s: String, var t: String):
        self.s = s^
        self.t = t^

def main():
    var b = Box("abc  ", "")
    b.t = String(b.s.rstrip())
    print(b.t)
