# expect: returned reference escapes storage outside its declared origin
# A view converted from a String temporary borrows the temporary's hidden
# frame-local slot, so it cannot be returned.
struct Box(Movable):
    var s: String

    def __init__(out self, var s: String):
        self.s = s^

    def view(ref self) -> StringSpan[origin_of(self)]:
        return String("temp")

def main():
    var b = Box(String("x"))
    print(b.view())
