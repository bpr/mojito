trait HasElem:
    comptime Element: Copyable & Movable = Int
    def get(self) -> Self.Element: ...

struct Box(HasElem):
    var v: Int
    def __init__(out self, v: Int):
        self.v = v
    def get(self) -> Self.Element:
        return self.v

struct Holder:
    comptime Element: Copyable & Movable = Int

def main():
    var x: Holder.Element = 4
    print(x)
    print(Box(3).get())
